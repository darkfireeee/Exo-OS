//! # syscall/dispatch.rs — Dispatch vers les handlers syscall
//!
//! Orchestre le pipeline complet d'un appel système :
//!
//! ```text
//! syscall_rust_handler(frame)
//!        │
//!        ├─ [0] Validation numéro syscall (bornes)
//!        ├─ [1] Fast-path check (getpid, gettid, clock_gettime…)
//!        │       └─ retour direct si traité (~40-150 cycles)
//!        ├─ [2] Compat translation (numéros Linux alternatifs)
//!        ├─ [3] Sélection handler dans table.rs (O(1))
//!        ├─ [4] Exécution handler
//!        └─ [5] Post-dispatch : signal pending check (RÈGLE SIGNAL-01)
//! ```
//!
//! ## Invariants de sécurité
//! - Aucun handler n'est appelé avec un numéro hors `[0, SYSCALL_TABLE_SIZE)`.
//! - Les registres `rdi..r9` ne sont jamais truqués avant le handler.
//! - Le résultat est toujours écrit dans `frame.rax` avant retour.
//!
//! ## Instrumentation
//! - `DISPATCH_TOTAL`       : nombre total de syscalls dispatués
//! - `DISPATCH_FAST_PATH`   : proportion traitée par fast-path
//! - `DISPATCH_SLOW_PATH`   : proportion traitée par la table
//! - `DISPATCH_ENOSYS`      : nombre de -ENOSYS retournés
//! - `DISPATCH_COMPAT`      : nombre de traductions compat appliquées
//! - `DISPATCH_LATENCY_NS`  : latence totale dispatch (TSC → ns, échantillon)
//!
//! ## RÈGLE CONTRAT UNSAFE (regle_bonus.md)
//! Tout `unsafe {}` est précédé d'un commentaire `// SAFETY:`.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::cpu::tsc::read_tsc;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::syscall::numbers::{SYSCALL_TABLE_SIZE, ENOSYS, is_valid_syscall};
use crate::syscall::fast_path::try_fast_path;
use crate::syscall::table::get_handler;
use crate::syscall::compat::linux::translate_linux_nr;

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs d'instrumentation
// ─────────────────────────────────────────────────────────────────────────────

static DISPATCH_TOTAL:      AtomicU64 = AtomicU64::new(0);
static DISPATCH_FAST_PATH:  AtomicU64 = AtomicU64::new(0);
static DISPATCH_SLOW_PATH:  AtomicU64 = AtomicU64::new(0);
static DISPATCH_ENOSYS:     AtomicU64 = AtomicU64::new(0);
static DISPATCH_COMPAT:     AtomicU64 = AtomicU64::new(0);
/// Somme des latences dispatch (cycles TSC). Échantillonné 1/256.
static DISPATCH_LATENCY_CYC: AtomicU64 = AtomicU64::new(0);

/// Snapshot des compteurs de dispatch.
#[derive(Copy, Clone, Debug, Default)]
pub struct DispatchStats {
    pub total:      u64,
    pub fast_path:  u64,
    pub slow_path:  u64,
    pub enosys:     u64,
    pub compat:     u64,
    pub latency_cyc: u64,
}

/// Retourne un snapshot instantané des compteurs.
pub fn dispatch_stats() -> DispatchStats {
    DispatchStats {
        total:       DISPATCH_TOTAL.load(Ordering::Relaxed),
        fast_path:   DISPATCH_FAST_PATH.load(Ordering::Relaxed),
        slow_path:   DISPATCH_SLOW_PATH.load(Ordering::Relaxed),
        enosys:      DISPATCH_ENOSYS.load(Ordering::Relaxed),
        compat:      DISPATCH_COMPAT.load(Ordering::Relaxed),
        latency_cyc: DISPATCH_LATENCY_CYC.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée principal du dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch l'appel système décrit par `frame`.
///
/// Cette fonction est appelée depuis `arch::x86_64::syscall::syscall_rust_handler`
/// (ou directement depuis le nouveau `syscall/mod.rs` qui court-circuite l'ancien
/// handler arch/).
///
/// ## Contrat
/// - `frame` est une `SyscallFrame` validée par le stub ASM `syscall_entry_asm`.
/// - GS est le GS kernel (SWAPGS effectué dans le stub).
/// - La préemption est ACTIVE (pas dans un ISR).
///
/// ## Pipeline d'exécution
/// 1. Mesure TSC début (instrumentation)
/// 2. Lecture des 6 arguments depuis la frame
/// 3. Validation du numéro syscall
/// 4. Essai fast-path (getpid, gettid, yield, clock_gettime, …)
/// 5. Traduction numéro compat Linux si nécessaire
/// 6. Lookup handler dans `table::get_handler(nr)`
/// 7. Appel handler
/// 8. Écriture du résultat dans `frame.rax`
/// 9. Check signal pending (RÈGLE SIGNAL-01)
/// 10. Mesure TSC fin (instrumentation)
#[no_mangle]
pub fn dispatch(frame: &mut SyscallFrame) {
    // ── [0] Instrumentation début ──────────────────────────────────────────
    // SAFETY: RDTSC est non-privilégié, aucun effet de bord.
    let tsc_start = unsafe { read_tsc() };
    DISPATCH_TOTAL.fetch_add(1, Ordering::Relaxed);

    // ── [1] Extraction des arguments ───────────────────────────────────────
    let nr   = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10;
    let arg5 = frame.r8;
    let arg6 = frame.r9;

    // ── [2] Validation du numéro syscall ───────────────────────────────────
    if !is_valid_syscall(nr) {
        DISPATCH_ENOSYS.fetch_add(1, Ordering::Relaxed);
        frame.rax = ENOSYS as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [3] Fast-path  ─────────────────────────────────────────────────────
    // Couvre les syscalls haute fréquence sans allocation ni verrou.
    if let Some(result) = try_fast_path(nr, arg1, arg2, arg3, arg4, arg5, arg6) {
        DISPATCH_FAST_PATH.fetch_add(1, Ordering::Relaxed);
        frame.rax = result as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [4] Traduction numéro compat Linux ─────────────────────────────────
    // Certains numéros Linux peuvent avoir un mapping alternatif dans Exo-OS.
    // Par ex. un numéro retiré de Linux qui est remappé vers l'équivalent Exo-OS.
    let effective_nr = match translate_linux_nr(nr) {
        Some(translated) => {
            DISPATCH_COMPAT.fetch_add(1, Ordering::Relaxed);
            translated
        }
        None => nr,
    };

    // ── [5] Slow-path : lookup dans la table ───────────────────────────────
    DISPATCH_SLOW_PATH.fetch_add(1, Ordering::Relaxed);
    let handler = get_handler(effective_nr);

    // ── [6] Exécution du handler ───────────────────────────────────────────
    let result = handler(arg1, arg2, arg3, arg4, arg5, arg6);

    // ── [7] Comptabilisation d'une erreur ENOSYS ───────────────────────────
    if result == ENOSYS {
        DISPATCH_ENOSYS.fetch_add(1, Ordering::Relaxed);
    }

    // ── [8] Écriture du résultat ───────────────────────────────────────────
    frame.rax = result as u64;

    // ── [9] Post-dispatch : signal pending + instrumentation ──────────────
    post_dispatch(frame, tsc_start);
}

// ─────────────────────────────────────────────────────────────────────────────
// Post-dispatch : signal pending + mesure latence
// ─────────────────────────────────────────────────────────────────────────────

/// Actions effectuées après chaque dispatch.
///
/// 1. Vérifie le flag `signal_pending` du TCB courant.
///    Si posé → appelle `process::signal::delivery::handle_pending_signals()`.
///    (RÈGLE SIGNAL-01 DOC1 : livraison au retour userspace UNIQUEMENT)
/// 2. Échantillonne la latence dispatch (1/256) pour éviter la contention atomique.
#[inline]
fn post_dispatch(frame: &mut SyscallFrame, tsc_start: u64) {
    // ── Livraison de signaux pending (RÈGLE SIGNAL-01) ────────────────────
    check_and_deliver_signals(frame);

    // ── Instrumentation latence (échantillon 1/256) ───────────────────────
    // SAFETY: RDTSC est non-privilégié, sans effet de bord.
    let tsc_end = unsafe { read_tsc() };
    let elapsed = tsc_end.saturating_sub(tsc_start);
    // Échantillonnage : bit 7 de tsc_end pour éviter le modulo coûteux.
    if (tsc_end & 0xFF) == 0 {
        DISPATCH_LATENCY_CYC.fetch_add(elapsed, Ordering::Relaxed);
    }
}

/// Vérifie et livre les signaux pending avant le retour userspace.
///
/// ## RÈGLE SIGNAL-01 / SIGNAL-02 (DOC1)
/// - `arch/` orchestre (lit le flag, appelle process/signal/).
/// - `process/signal/delivery` livre effectivement.
/// - `scheduler/` ne livre jamais directement.
///
/// ## Implémentation
/// Lit `gs:[0x20]` → pointeur TCB → champ `signal_pending`.
/// Si posé → appelle `process::signal::delivery::handle_pending_signals()`.
/// La livraison modifie la frame si elle installe un handler userspace.
#[inline]
fn check_and_deliver_signals(frame: &mut SyscallFrame) {
    // SAFETY: GS kernel actif dans ce contexte (SWAPGS dans le stub ASM).
    // gs:[0x20] contient le pointeur TCB, potentiellement nul si pas encore
    // initialisé (cas du kernel avant le premier fork → aucun signal possible).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem)
        );
    }

    if tcb_ptr == 0 {
        // Aucun thread courant (boot / idle) → pas de signal.
        return;
    }

    // ── Lecture du flag signal_pending ────────────────────────────────────
    // Le champ `signal_pending` est un AtomicBool dans ThreadControlBlock.
    // Son offset dans le TCB est FIXE (vérifié statiquement dans task.rs).
    //
    // Layout TCB (docs/refonte/DOC3 + task.rs) :
    // Offset 0..4    = tid (u32)
    // Offset 4..8    = pid (u32)
    // Offset 8       = state (u8)
    // ... voir task.rs pour le layout exact
    //
    // Le flag signal_pending est à l'offset défini par TCB_SIGNAL_PENDING_OFFSET
    // (constante within scheduler/core/task.rs, exportée dans la réexportation publique).
    //
    // SAFETY: tcb_ptr est non-nul et pointe vers un ThreadControlBlock valide
    // (construit par lifecycle/create.rs et maintenu par le scheduler).
    use crate::scheduler::core::task::ThreadControlBlock;
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };

    // Vérifier le flag avec Acquire pour synchroniser avec l'écriture signal_pending
    // effectuée par process/signal/ (Ordering::Release s'y applique).
    if !tcb.signal_pending.load(core::sync::atomic::Ordering::Acquire) {
        return;
    }

    // ── Livraison des signaux ─────────────────────────────────────────────
    // RÈGLE SIGNAL-02 (DOC1) : arch/x86_64/syscall.rs orchestre,
    // process::signal::delivery livre.
    //
    // La fonction handle_pending_signals() :
    // 1. Lit la queue de signaux du thread/process
    // 2. Pour chaque signal à livrer, appelle le handler userspace ou
    //    applique l'action par défaut (terminate, stop, ignore, continue).
    // 3. Si un handler userspace est installé, modifie la SyscallFrame
    //    pour que SYSRETQ retourne vers le signal trampolineau lieu
    //    de l'instruction qui a fait le syscall.
    //
    // SAFETY: frame est une SyscallFrame valide construite par le stub ASM.
    // tcb est valide (vérifié ci-dessus). L'appel est dans le contexte kernel
    // avec IRQs actives (les signaux peuvent potentiellement être ré-entrants
    // via SIGINT etc., mais handle_pending_signals() utilise un masque).
    let frame_ptr: *mut SyscallFrame = frame as *mut SyscallFrame;
    unsafe {
        crate::process::signal::delivery::handle_pending_signals(tcb_ptr as *mut _, frame_ptr as *mut _);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrappers de test / diagnostic
// ─────────────────────────────────────────────────────────────────────────────

/// Réinitialise tous les compteurs de dispatch à zéro.
/// Utilisé par les tests et le démarrage d'un profil.
pub fn reset_dispatch_stats() {
    DISPATCH_TOTAL.store(0, Ordering::Relaxed);
    DISPATCH_FAST_PATH.store(0, Ordering::Relaxed);
    DISPATCH_SLOW_PATH.store(0, Ordering::Relaxed);
    DISPATCH_ENOSYS.store(0, Ordering::Relaxed);
    DISPATCH_COMPAT.store(0, Ordering::Relaxed);
    DISPATCH_LATENCY_CYC.store(0, Ordering::Relaxed);
}

/// Invoque directement un syscall numéro `nr` avec les arguments donnés,
/// sans passer par la frame ASM. Utilisé par les tests internes.
///
/// Ne mesure pas la latence et ne déclenche pas la livraison de signaux.
#[cfg(any(test, feature = "syscall_test"))]
pub fn invoke_direct(nr: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64, arg6: u64) -> i64 {
    if !is_valid_syscall(nr) { return ENOSYS; }
    if let Some(r) = try_fast_path(nr, arg1, arg2, arg3, arg4, arg5, arg6) {
        return r;
    }
    let effective_nr = translate_linux_nr(nr).unwrap_or(nr);
    let handler = get_handler(effective_nr);
    handler(arg1, arg2, arg3, arg4, arg5, arg6)
}
