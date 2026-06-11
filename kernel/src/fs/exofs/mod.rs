// ExoFS — API publique du module
// Ring 0, no_std
// Responsabilités : exofs_init(), exofs_register_fs()

pub mod audit;
pub mod cache;
pub mod compress;
pub mod core;
pub mod crypto;
pub mod dedup;
pub mod epoch;
pub mod export;
pub mod gc;
pub mod io;
pub mod numa;
pub mod objects;
pub mod observability;
pub mod path;
pub mod posix_bridge;
pub mod quota;
pub mod recovery;
pub mod relation;
pub mod snapshot;
pub mod storage;
pub mod syscall;

/// Tests unitaires, intégration, fuzz (spec 2.0)
#[cfg(test)]
pub mod test_support;
#[cfg(test)]
pub mod tests;

use crate::fs::exofs::core::error::ExofsError;
use crate::fs::exofs::recovery::boot_recovery::boot_recovery_sequence;
use crate::fs::exofs::syscall::epoch_commit::{do_shutdown_commit, epoch_flags, EpochCommitArgs};
use crate::fs::exofs::cache::BLOB_CACHE;
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::scheduler::core::task::Priority;

use ::core::sync::atomic::{AtomicBool, Ordering};

const EXOFS_WRITEBACK_INTERVAL_NS: u64 = 5_000_000_000;

/// Macro de log noyau — no-op en attendant le système de log Ring-0.
#[allow(unused_macros)]
macro_rules! log_kernel {
    ($($arg:tt)*) => {};
}

/// État global du module ExoFS — initialisé une seule fois au boot
static EXOFS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialise ExoFS au boot du kernel.
/// Appelée par le VFS dispatcher après montage de la racine.
///
/// # Erreurs
/// - `ExofsError::AlreadyMounted` si appelée deux fois
/// - `ExofsError::CorruptSuperblock` si le superblock disque est invalide
/// - `ExofsError::RecoveryFailed` si la séquence de recovery échoue
pub fn exofs_init(disk_size_bytes: u64) -> Result<(), ExofsError> {
    if EXOFS_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ExofsError::AlreadyMounted);
    }

    // FIX-BOOT-FS : marqueurs de diagnostic E9 (préfixe '#') pour pinpointer un
    // éventuel blocage dans exofs_init. À retirer après validation boot.
    #[inline(always)]
    fn fsdbg(tag: u8) {
        // SAFETY: port 0xE9 = ISA debug device QEMU, sans effet mémoire.
        // `::core` car `core` est ambigu ici (module crate::fs::exofs::core).
        unsafe {
            ::core::arch::asm!("out 0xE9, al", in("al") b'#', options(nomem, nostack));
            ::core::arch::asm!("out 0xE9, al", in("al") tag, options(nomem, nostack));
        }
    }
    fsdbg(b'0');

    // Phase de montage du driver block virtio
    crate::fs::exofs::storage::virtio_adapter::init_global_disk();
    fsdbg(b'1');
    register_storage_flush_barrier();
    fsdbg(b'2');

    // Phase 1 : Recovery boot — sélectionne l'Epoch valide
    boot_recovery_sequence(disk_size_bytes).map_err(|_| ExofsError::RecoveryFailed)?;
    fsdbg(b'3');

    // Phase 2 : Enregistrement VFS (register_exofs_syscalls() — appelé via exofs_register_fs()).
    // Omis ici : enregistrement effectué après le boot via exofs_register_fs().

    // Phase 3 : Initialisation de la couche de compatibilité POSIX
    posix_bridge::posix_bridge_init()?;
    fsdbg(b'4');
    crate::process::lifecycle::exit::register_vfs_close_all_pid_hook(
        posix_bridge::vfs_close_all_pid,
    );
    crate::process::lifecycle::exec::register_close_exec_handles_hook(
        crate::syscall::fs_bridge::close_exec_handles_for_pid,
    );

    // Phase 4 : Threads background GC/writeback
    // Le kthread GC tourne en priorité basse — il appelle run_gc_two_phase() en boucle.
    // Le scheduler l'interrompt entre les cycles.
    let gc_params = KthreadParams {
        name: "exofs-gc",
        entry: exofs_gc_kthread,
        arg: 0,
        target_cpu: 0,
        priority: Priority(130), // nice +10: background, but not starved behind idle.
    };
    // Ignorer l'erreur si le scheduler n'est pas encore actif au boot
    let _ = create_kthread(&gc_params);
    fsdbg(b'5');
    let writeback_params = KthreadParams {
        name: "exofs-writeback",
        entry: exofs_writeback_kthread,
        arg: 0,
        target_cpu: 0,
        priority: Priority(128),
    };
    let _ = create_kthread(&writeback_params);
    fsdbg(b'6');

    log_kernel!(
        "[exofs] initialisé — disk_size={} MB",
        disk_size_bytes / (1024 * 1024)
    );

    Ok(())
}

/// Fonction d'entrée du kthread GC ExoFS.
///
/// S'exécute en boucle au niveau de priorité basse.
/// À chaque passage : récupère les blobs orphelins d'au moins 2 epochs de retard.
fn exofs_gc_kthread(_arg: usize) -> ! {
    // Background GC must never win the first userspace handoff. It only scans
    // after epochs have matured; before that it backs off without touching the
    // global blob cache.
    gc_backoff();

    loop {
        let current_epoch = crate::fs::exofs::syscall::epoch_commit::current_epoch();
        if current_epoch > 2 {
            // Lance un cycle GC complet (scan + collect) pour les epochs âgées de > 2.
            let epoch_threshold = current_epoch - 2;
            let _ = crate::fs::exofs::syscall::gc_trigger::run_gc_two_phase(epoch_threshold);
        }

        gc_backoff();
    }
}

fn exofs_writeback_kthread(_arg: usize) -> ! {
    gc_backoff();

    loop {
        let _ = exofs_writeback_dirty();
        if !crate::scheduler::timer::sleep_ns(EXOFS_WRITEBACK_INTERVAL_NS) {
            gc_backoff();
        }
    }
}

fn exofs_writeback_dirty() -> Result<(), ExofsError> {
    // FIX-EXOFS-CORE-1 (AUDIT-EXOFS §2) : le writeback ne persiste plus les blobs
    // bruts isolément — il déclenche un commit d'epoch transactionnel complet
    // (flush des blobs dirty + journal + EpochRoot/Record + barrières NVMe), seul
    // garant de l'atomicité et d'un état recouvrable au reboot. `commit_current_
    // epoch` court-circuite proprement s'il n'y a rien à committer ou si un commit
    // concourant (fsync/sync) est déjà en cours.
    if BLOB_CACHE.collect_dirty().is_empty() {
        return Ok(());
    }
    match crate::fs::exofs::syscall::epoch_commit::commit_current_epoch() {
        Ok(_) | Err(ExofsError::CommitInProgress) => Ok(()),
        Err(e) => Err(e),
    }
}

fn gc_backoff() {
    let mut yields = 0usize;
    while yields < 128 {
        crate::syscall::fast_path::sys_sched_yield();
        yields += 1;
    }
}

fn register_storage_flush_barrier() {
    crate::fs::exofs::epoch::epoch_barriers::register_nvme_flush_fn(
        crate::fs::exofs::storage::virtio_adapter::flush_global_disk,
    );
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
        flags: epoch_flags::FORCE,
        _pad: 0,
        epoch_id: 0, // 0 = epoch courante
        checksum: 0, // pas de vérification checksum au shutdown
        hints: 0,
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
