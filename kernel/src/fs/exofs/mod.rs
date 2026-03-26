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
use crate::fs::exofs::syscall::epoch_commit::{do_shutdown_commit, EpochCommitArgs, epoch_flags};
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::scheduler::core::task::Priority;

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

    // Phase de montage du driver block virtio
    crate::fs::exofs::storage::virtio_adapter::init_global_disk();

    // Phase 1 : Recovery boot — sélectionne l'Epoch valide
    boot_recovery_sequence(disk_size_bytes)
        .map_err(|_| ExofsError::RecoveryFailed)?;

    // Phase 2 : Enregistrement VFS (register_exofs_syscalls() — appelé via exofs_register_fs()).
    // Omis ici : enregistrement effectué après le boot via exofs_register_fs().

    // Phase 3 : Initialisation de la couche de compatibilité POSIX
    posix_bridge::posix_bridge_init()?;

    // Phase 4 : Threads background GC/writeback
    // Le kthread GC tourne en priorité basse — il appelle run_gc_two_phase() en boucle.
    // Le scheduler l'interrompt entre les cycles.
    let gc_params = KthreadParams {
        name:       "exofs-gc",
        entry:      exofs_gc_kthread,
        arg:        0,
        target_cpu: 0,
        priority:   Priority::IDLE,  // priorité basse — GC ne doit pas bloquer les I/O
    };
    // Ignorer l'erreur si le scheduler n'est pas encore actif au boot
    let _ = create_kthread(&gc_params);

    log_kernel!("[exofs] initialisé — disk_size={} MB",
        disk_size_bytes / (1024 * 1024));

    Ok(())
}

/// Fonction d'entrée du kthread GC ExoFS.
///
/// S'exécute en boucle au niveau de priorité basse.
/// À chaque passage : récupère les blobs orphelins d'au moins 2 epochs de retard.
fn exofs_gc_kthread(_arg: usize) -> ! {
    loop {
        // Lance un cycle GC complet (scan + collect) pour les epochs âgées de > 2.
        let epoch_threshold = crate::fs::exofs::syscall::epoch_commit::current_epoch()
            .saturating_sub(2);
        let _ = crate::fs::exofs::syscall::gc_trigger::run_gc_two_phase(epoch_threshold);

        // Yield le CPU via sys_sched_yield (fast-path) pour ne pas monopoliser le scheduler.
        crate::syscall::fast_path::sys_sched_yield();
    }
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

    // Commit l'epoch courante (force=true, flush tous les blobs en attente).
    // Utilise l'API interne do_shutdown_commit() qui ne passe pas par userspace.
    let commit_args = EpochCommitArgs {
        flags:    epoch_flags::FORCE,
        _pad:     0,
        epoch_id: 0,  // 0 = epoch courante
        checksum: 0,  // pas de vérification checksum au shutdown
        hints:    0,
    };
    // Ignorer une erreur CommitInProgress (un commit concourant finira le travail).
    match do_shutdown_commit(&commit_args) {
        Ok(_) | Err(ExofsError::CommitInProgress) => {}
        Err(e) => return Err(e),
    }

    EXOFS_INITIALIZED.store(false, Ordering::Release);
    log_kernel!("[exofs] arrêt propre effectué");
    Ok(())
}
