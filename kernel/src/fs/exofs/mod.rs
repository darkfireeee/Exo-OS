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

/// FIX-F1 : déverrouille le volume ExoFS s'il est chiffré, en installant la clé
/// de volume dérivée de `passphrase`. À appeler au **montage** (après
/// `boot_recovery_sequence`) une fois qu'une passphrase est disponible.
///
/// - `Ok(true)`  : volume chiffré déverrouillé (chiffrement-at-rest actif).
/// - `Ok(false)` : volume NON chiffré → no-op (chemin en clair, aucune régression).
/// - `Err(..)`   : passphrase incorrecte / superblock corrompu.
///
/// ⚠️ La **source** de la passphrase (paramètre de boot `exofs.key=` / clé scellée
/// TPM) est une décision de déploiement — délibérément NON câblée par défaut, pour
/// ne pas embarquer une passphrase en dur (= fausse sécurité). Voir
/// `docs/SECURITE/AUDIT-100-PERCENT.md` (F1). Lecture+déchiffrement du volume sont
/// déjà entièrement implémentés (`crypto::at_rest`, partagé avec mkfs via `exo-fscrypt`).
pub fn unlock_encrypted_volume(passphrase: &[u8]) -> Result<bool, ExofsError> {
    use crate::fs::exofs::crypto::at_rest;
    use crate::fs::exofs::storage::superblock::{ExoSuperblockDisk, SUPERBLOCK_DISK_SIZE};
    use crate::fs::exofs::storage::virtio_adapter;

    if !virtio_adapter::has_global_disk() {
        return Ok(false);
    }
    let block_size = virtio_adapter::with_global_disk(|d| Ok(d.block_size()))? as usize;
    let read_len = block_size.max(SUPERBLOCK_DISK_SIZE);
    let mut block = alloc::vec![0u8; read_len];
    virtio_adapter::with_global_disk(|d| d.read_block(0, &mut block[..block_size]))?;
    let sb = ExoSuperblockDisk::from_bytes(&block[..SUPERBLOCK_DISK_SIZE])?;
    if !sb.is_encrypted() {
        return Ok(false);
    }
    let wrapped = sb
        .wrapped_volume_key()
        .ok_or(ExofsError::CorruptedStructure)?;
    at_rest::install_volume_key_from_wrapped(&wrapped, passphrase)?;
    Ok(true)
}

/// Initialise ExoFS au boot du kernel.
/// Appelée par le VFS dispatcher après montage de la racine.
///
/// # Erreurs
/// - `ExofsError::AlreadyMounted` si appelée deux fois
/// - `ExofsError::CorruptSuperblock` si le superblock disque est invalide
/// - `ExofsError::RecoveryFailed` si la séquence de recovery échoue
pub fn exofs_init(mut disk_size_bytes: u64) -> Result<(), ExofsError> {
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

    // Repli ATA/IDE (PIO, 0x1F0 maître) si virtio-blk absent — Bochs (qui n'émule
    // ni virtio ni AHCI/NVMe) ou QEMU machine `pc`. Permet de lire le rootfs ExoFS
    // depuis un disque IDE legacy pour le diagnostic #25 sous Bochs.
    if !crate::fs::exofs::storage::virtio_adapter::has_global_disk() {
        fsdbg(b'I');
        if crate::fs::exofs::storage::ata_pio::init_global_disk_ata() {
            fsdbg(b'i');
        }
    }

    // FIX-GPT : si le disque est partitionné GPT, localiser la partition ExoFS
    // ROOT par son type-GUID et décaler toute l'I/O ExoFS vers son LBA de début
    // (parseur partagé `exo-partition`). ADDITIF : disque brut / MBR legacy / GPT
    // sans partition ROOT → aucun décalage, le superblock reste lu au LBA 0
    // (comportement des images mkfs actuelles, zéro régression). Toute erreur de
    // parsing (CRC/signature) retombe sur LBA 0.
    if let Some(rp) = crate::fs::exofs::storage::partition_scan::resolve_exofs_partition() {
        // Le volume ExoFS = partition ROOT : la taille effective (utilisée par le
        // boot recovery pour borner ses lectures) devient celle de la partition.
        let bs = crate::fs::exofs::storage::virtio_adapter::current_global_disk()
            .map(|d| d.block_size() as u64)
            .unwrap_or(512);
        disk_size_bytes = rp.sectors.saturating_mul(bs);
    }
    fsdbg(b'g');

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
