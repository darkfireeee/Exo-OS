// kernel/src/ipc/sync/futex.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FUTEX IPC — SHIM DÉLÉGATION PURE → memory::utils::futex_table
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE IPC-02 : CE MODULE NE CONTIENT AUCUNE LOGIQUE FUTEX LOCALE.
//   Délégation pure à memory::utils::futex_table::FUTEX_TABLE.
//   La table futex UNIQUE réside en memory/ (COUCHE 0) et est partagée
//   par tous les sous-systèmes (ipc/, scheduler/, process/).
//
//   RAISON : point de vérité unique — évite races cross-subsystems.
//            périmètre de preuve TLA+/Coq limité à la table mémoire.
//
// Ce module fournit :
//   • FutexKey    — clé IPC (adresse virtuelle physmap de l'AtomicU32)
//   • futex_wait  — WAIT avec spin-poll sur waiter.woken issu de memory/
//   • futex_wake  — WAKE (délègue directement)
//   • futex_stats — statistiques lues depuis memory::utils::FUTEX_STATS
//
// La gestion des buckets, la liste chaînée de waiters et la logique wake
// résident EXCLUSIVEMENT dans memory::utils::futex_table.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, Ordering};

use crate::ipc::core::types::IpcError;
use crate::memory::utils::futex_table::{
    futex_cancel as mem_futex_cancel, futex_requeue as mem_futex_requeue,
    futex_wait as mem_futex_wait, futex_wake as mem_futex_wake, futex_wake_n as mem_futex_wake_n,
    FutexWaitResult, FutexWaiter, WakeFn, FUTEX_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// FutexKey — clé identifiant un futex IPC par adresse virtuelle (physmap)
// ─────────────────────────────────────────────────────────────────────────────

/// Clé d'un futex IPC = adresse virtuelle de l'AtomicU32 partagé.
/// Pour un AtomicU32 dans une page SHM, l'adresse virtuelle physmap
/// est partagée entre tous les espaces d'adressage (même physmap).
/// Compatible avec memory::utils::futex_table (virt_addr comme clé).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct FutexKey(pub u64);

impl FutexKey {
    /// Construit une clé depuis une référence à un AtomicU32.
    #[inline(always)]
    pub fn from_addr(addr: &AtomicU32) -> Self {
        Self(addr as *const _ as u64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WaiterState — résultat d'un futex_wait
// ─────────────────────────────────────────────────────────────────────────────

/// Cause du réveil d'un waiter IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WaiterState {
    /// Réveillé normalement par futex_wake().
    Woken = 0,
    /// *addr ≠ expected — pas de blocage (retour immédiat).
    ValueMismatch = 1,
    /// Timeout ou annulation (futex_cancel).
    Cancelled = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// FutexIpcStats — snapshot des compteurs depuis memory/utils/FUTEX_STATS
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques futex IPC — miroir de memory::utils::futex_table::FUTEX_STATS.
#[derive(Debug, Clone, Copy)]
pub struct FutexIpcStats {
    /// Nombre d'appels WAIT depuis le boot.
    pub waits_total: u64,
    /// Nombre d'appels WAKE depuis le boot.
    pub wakes_total: u64,
    /// Nombre de timeouts (futex_cancel suite à spin_max dépassé).
    pub timeouts_total: u64,
    /// Nombre de WAIT où *addr ≠ expected (retour immédiat).
    pub value_mismatches: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonction de réveil IPC — délègue à sched_hooks::wake_thread (scheduler)
// ─────────────────────────────────────────────────────────────────────────────

/// WakeFn injectée dans FutexWaiter : réveille réellement le thread via scheduler.
/// Signature : fn(tid: u64, code: i32) — compatible avec memory::utils::futex_table.
fn ipc_futex_wake_fn(tid: u64, _code: i32) {
    super::sched_hooks::wake_thread(tid as u32);
}

/// No-op de réveil (avant installation des hooks ou pour les callers sans scheduler).
#[allow(dead_code)]
fn nop_wake_fn(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// API publique — SHIM vers memory::utils::futex_table
// ─────────────────────────────────────────────────────────────────────────────

/// WAIT IPC : si *addr == expected, enfile ce thread dans la table globale
/// memory::utils::futex_table puis spin-poll jusqu'à réveil ou timeout.
///
/// DÉLÈGUE À : memory::utils::futex_table::futex_wait() (enfilement)
///             puis spin-poll sur waiter.woken (AtomicBool du waiter globale)
///
/// # Paramètres
/// - `addr`      : référence vers l'AtomicU32 partagé dans la page SHM.
/// - `key`       : FutexKey = adresse virtuelle de `addr` (physmap).
/// - `expected`  : valeur attendue dans `*addr`.
/// - `thread_id` : TID du thread (pour corrélation wake ciblé).
/// - `spin_max`  : tours de spin max avant timeout (0 = infini).
/// - `wake_fn`   : callback scheduler injecté ; `None` → nop_wake_fn.
///
/// # Safety
/// - `addr` doit pointer vers un u32 atomique valide dans une page SHM.
/// - La durée de vie du waiter alloué sur la pile est garantie par le
///   spin-loop : la fonction ne retourne pas tant que le waiter est actif.
pub unsafe fn futex_wait(
    _addr: &AtomicU32,
    key: FutexKey,
    expected: u32,
    thread_id: u32,
    spin_max: u64,
    wake_fn: Option<WakeFn>,
) -> Result<WaiterState, IpcError> {
    // Utiliser ipc_futex_wake_fn par défaut : réveille réellement le thread.
    // Si le caller fournit une wake_fn explicite, l'utiliser à la place.
    let wfn = wake_fn.unwrap_or(ipc_futex_wake_fn);

    // Allouer le FutexWaiter sur la pile — sa durée de vie = durée du wait.
    let mut waiter = FutexWaiter::new(key.0, expected, thread_id as u64, wfn);
    let wptr = &mut waiter as *mut FutexWaiter;

    // Déléguer l'enfilement à memory::utils::futex_table (RÈGLE IPC-02).
    let result = mem_futex_wait(key.0, expected, wptr, wfn);

    match result {
        FutexWaitResult::ValueMismatch => Ok(WaiterState::ValueMismatch),

        FutexWaitResult::Waiting => {
            // Le waiter est dans le bucket de memory/utils/futex_table.
            // Stratégie : spin court puis blocage réel via sched_hooks.
            const SPIN_BEFORE_BLOCK: u64 = 64;
            let mut spins: u64 = 0;

            loop {
                core::hint::spin_loop();
                spins += 1;

                if waiter.woken.load(Ordering::Acquire) {
                    return Ok(WaiterState::Woken);
                }

                // Timeout explicite (spin_max non nul).
                if spin_max != 0 && spins >= spin_max {
                    mem_futex_cancel(wptr);
                    return Err(IpcError::Timeout);
                }

                // Après la phase de spin courte : bloquer via le scheduler.
                if spins >= SPIN_BEFORE_BLOCK {
                    // Vérifier à nouveau avant de bloquer (évite réveil manqué).
                    if waiter.woken.load(Ordering::Acquire) {
                        return Ok(WaiterState::Woken);
                    }
                    // Blocage réel — retourne après que ipc_futex_wake_fn a été appelée.
                    // RÈGLE PREEMPT-BLOCK (B6) : bloquer avec PreemptGuard actif = deadlock garanti.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "futex_wait: block_current() appelé avec PreemptGuard actif — deadlock garanti"
                    );
                    super::sched_hooks::block_current(thread_id);
                    // Réinitialiser le compteur pour la prochaine itération.
                    spins = 0;
                }
            }
        }
    }
}

/// WAKE IPC : réveille jusqu'à `n` threads attendant sur `key`.
///
/// DÉLÈGUE À : memory::utils::futex_table::futex_wake()
///
/// Retourne le nombre de threads effectivement réveillés.
///
/// # Safety
/// - `key` doit correspondre à des waiters valides dans la table globale.
pub unsafe fn futex_wake(key: FutexKey, n: u32) -> u32 {
    mem_futex_wake(key.0, n, 0)
}

/// WAKE ALL IPC : réveille tous les threads sur `key`.
///
/// DÉLÈGUE À : memory::utils::futex_table::futex_wake_n()
pub unsafe fn futex_wake_all(key: FutexKey) -> u32 {
    mem_futex_wake_n(key.0, u32::MAX)
}

/// CANCEL IPC : annule un waiter inscrit (timeout, fermeture canal).
///
/// DÉLÈGUE À : memory::utils::futex_table::futex_cancel()
///
/// # Safety : `waiter` doit être dans la table globale.
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter) {
    mem_futex_cancel(waiter);
}

/// REQUEUE IPC : réveille `max_wake` threads sur `src`, requeue `max_requeue`
/// vers `dst`. Utile pour les condvars (pthread_cond_broadcast IPC).
///
/// DÉLÈGUE À : memory::utils::futex_table::futex_requeue()
pub unsafe fn futex_requeue(
    src: FutexKey,
    dst: FutexKey,
    max_wake: u32,
    max_requeue: u32,
) -> (u32, u32) {
    mem_futex_requeue(src.0, dst.0, max_wake, max_requeue, 0)
}

/// Retourne un snapshot des statistiques depuis memory::utils::FUTEX_STATS.
/// La source de vérité est la table mémoire — pas de doublon local.
pub fn futex_stats() -> FutexIpcStats {
    FutexIpcStats {
        waits_total: FUTEX_STATS.wait_calls.load(Ordering::Relaxed),
        wakes_total: FUTEX_STATS.wake_calls.load(Ordering::Relaxed),
        timeouts_total: FUTEX_STATS.timeouts.load(Ordering::Relaxed),
        value_mismatches: FUTEX_STATS.value_mismatches.load(Ordering::Relaxed),
    }
}
