// kernel/src/process/thread/detach.rs
//
// pthread_detach — marque un thread comme détaché.
// Un thread détaché libère ses ressources automatiquement à la terminaison.
// Aucun join() n'est possible après detach().


use core::sync::atomic::Ordering;
use crate::process::core::tcb::ProcessThread;

/// Erreur de detach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachError {
    /// Déjà détaché.
    AlreadyDetached,
    /// Déjà joiné (join en cours).
    JoinInProgress,
    /// Pointeur invalide.
    InvalidThread,
}

/// Détache le thread cible.
/// Après un appel réussi, le thread libère ses ressources automatiquement.
///
/// # Safety
/// `target` doit pointer vers un ProcessThread valide.
pub fn thread_detach(target: *mut ProcessThread) -> Result<(), DetachError> {
    // SAFETY: target garanti valide par l'appelant.
    let thread = unsafe { &*target };

    // CAS : false → true (atomique).
    match thread.detached.compare_exchange(
        false, true,
        Ordering::AcqRel,
        Ordering::Relaxed,
    ) {
        Ok(_)  => Ok(()),
        Err(_) => Err(DetachError::AlreadyDetached),
    }
}

/// Vérifie si un thread est détaché.
#[inline(always)]
pub fn is_detached(thread: &ProcessThread) -> bool {
    thread.detached.load(Ordering::Relaxed)
}
