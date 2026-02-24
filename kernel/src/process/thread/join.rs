// kernel/src/process/thread/join.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// pthread_join / thread_join — attente de terminaison de thread (futex-based)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Mécanisme :
//   • ProcessThread::join_done : AtomicBool indiquant la fin.
//   • Joineur : poll join_done. Si faux → attend via wait_queue.
//   • Thread terminé : met join_result + join_done=true + wake_all.
//
// join() est CONSIDÉRÉ INTERRUPTIBLE par les signaux.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use crate::process::core::tcb::ProcessThread;
use crate::scheduler::core::task::ThreadControlBlock;
use crate::scheduler::sync::wait_queue::WaitQueue;

/// Table de join : une WaitQueue par TID possible.
/// Utilisée par do_exit_thread() pour réveiller les joineurs.
static JOIN_WAIT: WaitQueue = WaitQueue::new();

/// Erreur de join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    /// Le thread cible est détaché.
    Detached,
    /// Déjà joiné par un autre thread.
    AlreadyJoined,
    /// Interrompu par un signal.
    Interrupted,
    /// Pointeur thread invalide.
    InvalidThread,
}

/// Attend la terminaison du thread cible et récupère sa valeur de retour.
///
/// # Safety
/// `target` doit pointer vers un ProcessThread valide, non libéré.
/// `caller_tcb` doit être le TCB du thread appelant.
pub fn thread_join(
    target:     *const ProcessThread,
    caller_tcb: &ThreadControlBlock,
) -> Result<u64, JoinError> {
    // SAFETY: caller garantit que target est valide.
    let target = unsafe { &*target };

    // Vérifier que le thread n'est pas détaché.
    if target.detached.load(Ordering::Relaxed) {
        return Err(JoinError::Detached);
    }

    // Attendre la completion.
    // Boucle spurious-wakeup-safe.
    loop {
        if target.join_done.load(Ordering::Acquire) {
            let result = target.join_result.load(Ordering::Acquire);
            return Ok(result);
        }
        // Vérifier signal pending (EINTR).
        if caller_tcb.has_signal_pending() {
            return Err(JoinError::Interrupted);
        }
        // Bloquer sur la wait queue.
        // SAFETY: JOIN_WAIT utilise l'EmergencyPool (RÈGLE WAITQ-01).
        // caller_tcb est le TCB courant, pas d'alias &mut actif.
        unsafe {
            JOIN_WAIT.wait_interruptible(caller_tcb as *const _ as *mut _);
        }
    }
}

/// Réveille tous les joineurs d'un thread (appelé par do_exit_thread).
pub fn wake_joiners() {
    JOIN_WAIT.notify_all();
}
