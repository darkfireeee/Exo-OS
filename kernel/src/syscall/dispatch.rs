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


use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::cpu::tsc::read_tsc;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::syscall::numbers::{ENOSYS, is_valid_syscall};
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
    let tsc_start = read_tsc();
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

    // ── [5] Cas spécial rt_sigreturn — besoin d'accéder à la frame arch ────
    // SYS_RT_SIGRETURN (15) doit restaurer les registres directement dans frame
    // avant le retour SYSRETQ. Il ne peut pas passer par un handler normal.
    if effective_nr == crate::syscall::numbers::SYS_RT_SIGRETURN {
        handle_sigreturn_inplace(frame);
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [5b] Cas spécial fork — besoin de frame.rcx (child RIP) et frame.rsp ──
    if effective_nr == crate::syscall::numbers::SYS_FORK {
        let result = handle_fork_inplace(frame);
        frame.rax = result as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [5c] Cas spécial execve — modifie frame pour sauter au nouveau binaire ──
    if effective_nr == crate::syscall::numbers::SYS_EXECVE {
        handle_execve_inplace(frame);
        // frame.rax et frame.rcx sont déjà mis à jour par handle_execve_inplace.
        // Pas de post_dispatch après execve réussi (nouveau processus).
        return;
    }

    // ── [6] Slow-path : lookup dans la table ───────────────────────────────
    DISPATCH_SLOW_PATH.fetch_add(1, Ordering::Relaxed);
    let handler = get_handler(effective_nr);

    // ── [7] Exécution du handler ───────────────────────────────────────────
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
// Traitement spécial rt_sigreturn (SIG-13 / SIG-14)
// ─────────────────────────────────────────────────────────────────────────────

/// Traite `SYS_RT_SIGRETURN` directement depuis la frame arch.
///
/// Contrairement aux autres syscalls, `rt_sigreturn` doit modifier les registres
/// userspace (RIP, RSP, RFLAGS, RAX…) retournés par SYSRETQ. Le handler normal
/// n'a pas accès à la `SyscallFrame` — ce traitement spécial le permet.
///
/// ## Protocole (POSIX / SIG-13)
/// 1. RSP userspace au moment de SYSRETQ = sig_rsp + 8 (le `ret` du handler a
///    consommé `pretcode`).
/// 2. `sig_rsp = frame.user_rsp - 8`.
/// 3. UContext à `sig_rsp + SIGNAL_FRAME_UC_OFFSET`.
/// 4. Magic `0x5349_474E` vérifié constant-time (LAC-01 / SIG-14).
/// 5. Si magic OK : registres + signal_mask restaurés.
/// 6. RSP userspace mis à jour dans `gs:[0x08]` pour SYSRETQ.
///
/// En cas d'échec (magic invalide, adresse invalide) : le processus reçoit SIGSEGV
/// au retour vers userspace (RIP sera 0, qui est non-mappé).
fn handle_sigreturn_inplace(frame: &mut SyscallFrame) {
    use crate::process::signal::handler::{verify_and_extract_uc, SIGNAL_FRAME_UC_OFFSET};
    use crate::scheduler::core::task::ThreadControlBlock;

    // Le `ret` du handler a popped `pretcode` → RSP userspace = sig_rsp + 8.
    let sig_rsp = frame.rsp.wrapping_sub(8);
    let uc_ptr  = sig_rsp + SIGNAL_FRAME_UC_OFFSET;

    let regs = match verify_and_extract_uc(uc_ptr) {
        Some(r) => r,
        None => {
            // Magic invalide ou adresse corrompue : déclencher SIGSEGV.
            // On met RIP = 0 (non-mappé) → #PF Ring3 → SIGSEGV au processus.
            // On ne crashe jamais en Ring0 (ARCH-SYSRET V-35 respecté).
            frame.rcx = 0;
            frame.rax = (-crate::syscall::errno::EFAULT) as u64;
            return;
        }
    };

    // Restaurer les registres dans la frame arch (lus par SYSRETQ).
    // RIP de retour = rcx pour SYSRETQ.
    frame.rcx      = regs.rip;
    // RFLAGS userspace = r11 pour SYSRETQ.
    frame.r11      = regs.rflags & !0x100; // Clear TF (Trap Flag) par sécurité.
    frame.rax      = regs.rax;
    frame.rdi      = regs.rdi;
    frame.rsi      = regs.rsi;
    frame.rdx      = regs.rdx;
    frame.r8       = regs.r8;
    frame.r9       = regs.r9;
    // RSP userspace mis à jour dans gs:[0x08] ET dans frame.rsp.
    // SAFETY: GS kernel actif, gs:[0x08] = user_rsp slot du PerCpuData.
    unsafe {
        core::arch::asm!(
            "mov qword ptr gs:[0x08], {rsp}",
            rsp = in(reg) regs.rsp,
            options(nostack, nomem)
        );
    }
    frame.rsp = regs.rsp;

    // Restaurer le masque de signal dans le TCB.
    // SAFETY: gs:[0x20] = pointeur TCB courant (PerCpuData layout).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr != 0 {
        // Masque SIGKILL (bit 8) et SIGSTOP (bit 18) non-masquables (SIG-07).
        const NON_MASKABLE: u64 = (1u64 << 8) | (1u64 << 18);
        let safe_mask = regs.signal_mask & !NON_MASKABLE;
        // SAFETY: tcb_ptr est non-nul et valide (maintenu par le scheduler).
        let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
        tcb.signal_mask.store(safe_mask, core::sync::atomic::Ordering::Release);
    }
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
    let tsc_end = read_tsc();
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
    if !tcb.has_signal_pending() {
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
    // ── Conversion arch::SyscallFrame → delivery::SyscallFrame ───────────────
    // Les deux structs ont des noms de champs différents mais les mêmes valeurs.
    use crate::process::signal::delivery::{
        handle_pending_signals,
        SyscallFrame as DeliveryFrame,
    };
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;

    let pid = Pid(tcb.pid.0);
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None    => return,
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() { return; }

    let mut d_frame = DeliveryFrame {
        user_rip:    frame.rcx,     // RIP de retour (sauvé par SYSCALL hw dans RCX)
        user_rflags: frame.r11,     // RFLAGS (sauvé par SYSCALL hw dans R11)
        user_rsp:    frame.rsp,     // RSP userspace
        user_rax:    frame.rax,     // valeur de retour syscall
        user_rdi:    frame.rdi,
        user_rsi:    frame.rsi,
        user_rdx:    frame.rdx,
        user_rcx:    frame.rcx,     // userspace RCX = même que RIP retour (SYSCALL)
        user_r8:     frame.r8,
        user_r9:     frame.r9,
        user_cs:     0x1B,          // CS ring3 (non sauvé par SYSCALL mais requis)
        user_ss:     0x23,          // SS ring3
    };

    // SAFETY: thread_ptr maintenu par pcb, valide dans ce contexte.
    handle_pending_signals(unsafe { &mut *thread_ptr }, &mut d_frame);

    // Répercuter les modifications potentielles (ex. RIP redirigé, RSP vers sigaltstack).
    frame.rcx = d_frame.user_rip;
    frame.r11 = d_frame.user_rflags;
    frame.rax = d_frame.user_rax;
    frame.rdi = d_frame.user_rdi;
    frame.rsi = d_frame.user_rsi;
    frame.rdx = d_frame.user_rdx;
    frame.r8  = d_frame.user_r8;
    frame.r9  = d_frame.user_r9;

    // RSP userspace : mettre à jour gs:[0x08] si modifié (sigaltstack / setup_signal_frame).
    if d_frame.user_rsp != frame.rsp {
        let new_rsp = d_frame.user_rsp;
        // SAFETY: GS kernel actif, gs:[0x08] = user_rsp slot du PerCpuData.
        unsafe {
            core::arch::asm!(
                "mov qword ptr gs:[0x08], {rsp}",
                rsp = in(reg) new_rsp,
                options(nostack, nomem)
            );
        }
        frame.rsp = new_rsp;
    }

    let _ = frame_ptr; // silence warning
}

// ─────────────────────────────────────────────────────────────────────────────
// Traitement spécial fork (SYS_FORK = 57)
// ─────────────────────────────────────────────────────────────────────────────

/// Traite `SYS_FORK` directement depuis la frame arch.
///
/// Fork a besoin de `frame.rcx` (RIP de retour = point de bifurcation) et
/// `frame.rsp` (RSP userspace). Ces valeurs ne transitent pas dans les arguments
/// syscall normaux (rdi..r9), donc le handler normal ne peut pas les lire.
///
/// ## Protocole (POSIX fork)
/// 1. Lit TCB courant via `gs:[0x20]` → PID → PCB.
/// 2. Récupère le `ProcessThread` via `pcb.main_thread_ptr()`.
/// 3. Construit `ForkContext { child_rip: frame.rcx, child_rsp: frame.rsp }`.
/// 4. Appelle `do_fork()` — crée PCB/TCB fils, CoW, TLB flush, enqueue RunQueue.
/// 5. Retourne `child_pid` au parent (fils démarre via `fork_child_trampoline`).
fn handle_fork_inplace(frame: &SyscallFrame) -> i64 {
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::fork::{do_fork, ForkContext, ForkFlags, ForkError};
    use crate::syscall::errno::{EAGAIN, ENOMEM, EFAULT};

    // Lire TCB courant.
    // SAFETY: GS kernel actif, gs:[0x20] = pointeur TCB (PerCpuData).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        return EAGAIN;
    }

    // SAFETY: tcb_ptr non-nul et maintenu par le scheduler.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let pid = Pid(tcb.pid.0);

    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None    => return EAGAIN,
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return EAGAIN;
    }

    // SAFETY: thread_ptr maintenu par pcb, valide tant que pcb est dans PROCESS_REGISTRY.
    let thread = unsafe { &*thread_ptr };

    let ctx = ForkContext {
        parent_thread: thread,
        parent_pcb:    pcb,
        flags:         ForkFlags::default(),
        target_cpu:    tcb.current_cpu().0,
        child_rip:     frame.rcx,   // RIP de retour sauvé par SYSCALL hw
        child_rsp:     frame.rsp,   // RSP userspace sauvé au stub ASM
        parent_rflags: frame.r11,   // RFLAGS sauvés par SYSCALL hw — CORRECTION P2-02
    };

    match do_fork(&ctx) {
        Ok(result)  => result.child_pid.0 as i64,
        Err(e) => match e {
            ForkError::PidExhausted |
            ForkError::TidExhausted |
            ForkError::RegistryError |
            ForkError::InvalidCpu    => EAGAIN,
            ForkError::OutOfMemory |
            ForkError::AddressSpaceCloneFailed => ENOMEM,
            ForkError::NoAddrCloner  => EFAULT,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Traitement spécial execve (SYS_EXECVE = 59)
// ─────────────────────────────────────────────────────────────────────────────

/// Copie un tableau de chaînes null-terminé depuis l'espace utilisateur.
///
/// `argv_ptr` pointe vers un tableau de pointeurs `char*` terminé par NULL.
/// Retourne `Some(Vec<String>)` ou `None` si une adresse est invalide.
///
/// EXEC-01 : seule fonction autorisée à lire argv/envp depuis userspace.
fn copy_userspace_argv(
    argv_ptr: u64,
    max_args: usize,
) -> Option<alloc::vec::Vec<alloc::string::String>> {
    use alloc::string::String;
    use alloc::vec::Vec;
    use crate::syscall::validation::USER_ADDR_MAX;

    if argv_ptr == 0 { return Some(Vec::new()); }
    if argv_ptr >= USER_ADDR_MAX { return None; }

    let mut result: Vec<String> = Vec::new();

    for i in 0..max_args {
        // Lire le i-ème pointeur (u64) du tableau.
        let ptr_addr = match argv_ptr.checked_add(i as u64 * 8) {
            Some(a) if a < USER_ADDR_MAX => a,
            _ => return None,
        };

        // SAFETY: ptr_addr est une adresse userspace validée.
        let str_ptr: u64 = unsafe {
            core::ptr::read_volatile(ptr_addr as *const u64)
        };

        if str_ptr == 0 { break; }              // terminateur NULL du tableau
        if str_ptr >= USER_ADDR_MAX { return None; }

        // Lire la chaîne C octet par octet dans un vecteur heap.
        let mut bytes: alloc::vec::Vec<u8> = Vec::new();
        for j in 0..4095usize {
            let byte_addr = match str_ptr.checked_add(j as u64) {
                Some(a) if a < USER_ADDR_MAX => a,
                _ => return None,
            };
            // SAFETY: byte_addr est une adresse userspace validée.
            let byte = unsafe { core::ptr::read_volatile(byte_addr as *const u8) };
            if byte == 0 { break; }
            bytes.push(byte);
        }

        // Conversion UTF-8 permissive (remplace les octets invalides par U+FFFD).
        result.push(String::from_utf8_lossy(&bytes).into_owned());
    }

    Some(result)
}

/// Traite `SYS_EXECVE` directement depuis la frame arch.
///
/// En cas de succès, `do_execve()` remplace l'image du processus mais retourne
/// normalement en Rust. Ce handler met alors à jour la frame arch pour que
/// SYSRETQ saute directement au nouveau point d'entrée ELF.
///
/// ## Protocole
/// 1. Lit path depuis userspace via `frame.rdi`.
/// 2. Récupère TCB → PCB → ProcessThread.
/// 3. Appelle `do_execve()`.
/// 4. Succès : `frame.rcx = entry_point`, `frame.rsp = initial_rsp`, `frame.rax = 0`.
/// 5. Échec : `frame.rax = -errno`.
///
/// ## Note : argv/envp
/// Pour l'instant argv et envp sont transmis vides au `ElfLoader`.
/// Le câblage complet copy_from_user(argv/envp) est prévu en Phase 4 (ARGV-01).
fn handle_execve_inplace(frame: &mut SyscallFrame) {
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::exec::{do_execve, ExecError, ElfLoadError};
    use crate::syscall::validation::read_user_path;
    use crate::syscall::errno::*;

    // Lire le chemin depuis userspace.
    let user_path = match read_user_path(frame.rdi) {
        Ok(p)  => p,
        Err(e) => { frame.rax = e.to_errno() as u64; return; },
    };
    let path = match user_path.as_str() {
        Ok(s)  => s,
        Err(_) => { frame.rax = EFAULT as u64; return; },
    };

    // Lire TCB courant.
    // SAFETY: GS kernel actif.
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        frame.rax = EFAULT as u64;
        return;
    }

    // SAFETY: tcb_ptr maintenu par le scheduler.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let pid = Pid(tcb.pid.0);

    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None    => { frame.rax = EFAULT as u64; return; },
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        frame.rax = EAGAIN as u64;
        return;
    }

    // SAFETY: thread_ptr maintenu par pcb.
    let thread = unsafe { &mut *thread_ptr };

    // ARGV-01 (EXEC-01) : copier argv/envp depuis userspace avant do_execve().
    // frame.rsi = argv_ptr (tableau de char* null-terminé)
    // frame.rdx = envp_ptr (tableau de char* null-terminé)
    let argv_strings = match copy_userspace_argv(frame.rsi, 1024) {
        Some(v) => v,
        None    => { frame.rax = EFAULT as u64; return; }
    };
    let argv_refs: alloc::vec::Vec<&str> =
        argv_strings.iter().map(|s| s.as_str()).collect();

    let envp_strings = match copy_userspace_argv(frame.rdx, 4096) {
        Some(v) => v,
        None    => { frame.rax = EFAULT as u64; return; }
    };
    let envp_refs: alloc::vec::Vec<&str> =
        envp_strings.iter().map(|s| s.as_str()).collect();

    match do_execve(thread, pcb, &path, &argv_refs, &envp_refs) {
        Ok(()) => {
            // Succès : lire le nouveau point d'entrée depuis le ProcessThread mis à jour.
            let new_rip = thread.addresses.entry_point;
            let new_rsp = thread.addresses.initial_rsp;

            // Mettre à jour la frame pour SYSRETQ.
            frame.rcx = new_rip;            // RIP → nouveau point d'entrée ELF
            frame.rsp = new_rsp;            // RSP → nouvelle pile userspace
            frame.r11 = 0x0202;             // RFLAGS : IF=1, bit réservé=1
            frame.rax = 0;                  // "succès" (non retourné — SYSRETQ saute)

            // SYSRETQ lit gs:[0x08] pour restaurer le RSP userspace (stub ASM).
            // SAFETY: GS kernel actif, gs:[0x08] = user_rsp slot du PerCpuData.
            unsafe {
                core::arch::asm!(
                    "mov qword ptr gs:[0x08], {rsp}",
                    rsp = in(reg) new_rsp,
                    options(nostack, nomem)
                );
            }
        }
        Err(e) => {
            let errno: i64 = match e {
                ExecError::ElfLoadFailed(ElfLoadError::NotFound)  => ENOENT,
                ExecError::ElfLoadFailed(ElfLoadError::PermissionDenied)
                | ExecError::PermissionDenied                     => EACCES,
                ExecError::ElfLoadFailed(ElfLoadError::OutOfMemory) => ENOMEM,
                ExecError::ElfLoadFailed(ElfLoadError::InvalidElf) => EINVAL,
                ExecError::ArgListTooLong                          => E2BIG,
                ExecError::NameTooLong                             => EINVAL,
                ExecError::NoLoader                                => ENOSYS,
                _                                                  => ENOSYS,
            };
            frame.rax = errno as u64;
        }
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
