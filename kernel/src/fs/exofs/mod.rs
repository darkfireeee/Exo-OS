// ExoFS — API publique du module
// Ring 0, no_std
// Responsabilités : exofs_init(), exofs_register_fs()

pub mod core;
pub mod objects;
pub mod path;
pub mod epoch;
pub mod storage;
pub mod gc;
pub mod dedup;
pub mod compress;
pub mod crypto;
pub mod snapshot;
pub mod relation;
pub mod quota;
pub mod syscall;
pub mod posix_bridge;
pub mod io;
pub mod cache;
pub mod recovery;
pub mod export;
pub mod numa;
pub mod observability;
pub mod audit;

/// Tests unitaires, intégration, fuzz (spec 2.0)
#[cfg(test)]
pub mod tests;

use crate::fs::exofs::core::error::ExofsError;
use crate::fs::exofs::storage::superblock::SuperblockInMemory;
use crate::fs::exofs::recovery::boot_recovery::boot_recovery_sequence;

use alloc::sync::Arc;
use ::core::sync::atomic::{AtomicBool, Ordering};

/// Macro de log noyau — no-op en attendant le système de log Ring-0.
#[allow(unused_macros)]
macro_rules! log_kernel {
    ($($arg:tt)*) => {};
}

/// État global du module ExoFS — initialisé une seule fois au boot
static EXOFS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Référence globale au superblock actif (protégée par SpinLock dans SuperblockInMemory)
#[allow(dead_code)]
static mut EXOFS_SUPERBLOCK: Option<Arc<SuperblockInMemory>> = None;

/// Initialise ExoFS au boot du kernel.
/// Appelée par le VFS dispatcher après montage de la racine.
///
/// # Erreurs
/// - `ExofsError::AlreadyMounted` si appelée deux fois
/// - `ExofsError::CorruptSuperblock` si le superblock disque est invalide
/// - `ExofsError::RecoveryFailed` si la séquence de recovery échoue
pub fn exofs_init(disk_size_bytes: u64) -> Result<(), ExofsError> {
    if EXOFS_INITIALIZED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(ExofsError::AlreadyMounted);
    }

    // Phase 1 : Recovery boot — sélectionne l'Epoch valide
    boot_recovery_sequence(disk_size_bytes)
        .map_err(|_| ExofsError::RecoveryFailed)?;

    // Phase 2 : Enregistrement VFS (register_exofs_syscalls() — appelé via exofs_register_fs())
    // Omis ici : enregistrement effectué après le boot via exofs_register_fs().

    // Phase 3 : Initialisation de la couche de compatibilité POSIX
    posix_bridge::posix_bridge_init()?;

    // Phase 4 : Threads background GC/writeback — TODO: implémenter gc_thread et writeback
    // gc::gc_thread::start_gc_thread()?;
    // io::writeback::start_writeback_thread()?;

    log_kernel!("[exofs] initialisé — disk_size={} MB",
        disk_size_bytes / (1024 * 1024));

    Ok(())
}

/// Enregistre ExoFS dans la table VFS du kernel.
/// Retourne le pointeur sur les opérations VFS.
pub fn exofs_register_fs() -> Result<(), ExofsError> {
    posix_bridge::vfs_compat::register_exofs_vfs_ops()
}

/// Démontage propre : flush epoch, attente GC, sync superblock.
pub fn exofs_shutdown() -> Result<(), ExofsError> {
    if !EXOFS_INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }

    // Commit l'epoch courant avant de s'arrêter
    // commit_current_epoch() — TODO: passer CommitInput après refactoring epoch_commit

    // Sync superblock miroirs — TODO: implémenter sync_all_mirrors
    // storage::superblock_backup::sync_all_mirrors()?;

    EXOFS_INITIALIZED.store(false, Ordering::Release);
    log_kernel!("[exofs] arrêt propre effectué");
    Ok(())
}
