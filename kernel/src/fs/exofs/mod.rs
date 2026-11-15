// ExoFS — Public module API
// Ring 0, no_std
// Responsibilities: exofs_init(), exofs_register_fs()

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

/// Unit tests, integration, fuzz (spec 2.0)
#[cfg(test)]
pub mod test_support;
#[cfg(test)]
pub mod tests;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::error::ExofsError;
use crate::fs::exofs::recovery::boot_recovery::boot_recovery_sequence;
use crate::fs::exofs::syscall::epoch_commit::{do_shutdown_commit, epoch_flags, EpochCommitArgs};
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::scheduler::core::task::Priority;

use crate::rust_core::sync::atomic::{AtomicBool, Ordering};

const EXOFS_WRITEBACK_INTERVAL_NS: u64 = 5_000_000_000;

/// Kernel log macro — no-op awaiting the Ring-0 logging system.
#[allow(unused_macros)]
macro_rules! log_kernel {
    ($($arg:tt)*) => {};
}

/// Global ExoFS module state — initialized once at boot
static EXOFS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// FIX-F1: unlocks the ExoFS volume if encrypted, installing the volume
/// key derived from `passphrase`. To be called at **mount** time (after
/// `boot_recovery_sequence`) once a passphrase is available.
///
/// - `Ok(true)`  : encrypted volume unlocked (encryption-at-rest active).
/// - `Ok(false)` : volume NOT encrypted -> no-op (plaintext path, zero regression).
/// - `Err(..)`   : incorrect passphrase / corrupted superblock.
///
/// ⚠️ The **source** of the passphrase (boot parameter `exofs.key=` / TPM-sealed
/// key) is a deployment decision — deliberately NOT hardwired by default, to
/// avoid embedding a hardcoded passphrase (= false security). See
/// `docs/SECURITE/AUDIT-100-PERCENT.md` (F1). Volume reading+decryption are
/// already fully implemented (`crypto::at_rest`, shared with mkfs via `exo-fscrypt`).
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

/// Initializes ExoFS at kernel boot.
/// Called by VFS dispatcher after root mount.
///
/// # Errors
/// - `ExofsError::AlreadyMounted` if called twice
/// - `ExofsError::CorruptSuperblock` if the disk superblock is invalid
/// - `ExofsError::RecoveryFailed` if the recovery sequence fails
pub fn exofs_init(mut disk_size_bytes: u64) -> Result<(), ExofsError> {
    if EXOFS_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ExofsError::AlreadyMounted);
    }

    // FIX-BOOT-FS: E9 diagnostic markers ('#' prefix) to pinpoint any
    // lockup in exofs_init. To be removed after boot validation.
    #[inline(always)]
    fn fsdbg(tag: u8) {
        // SAFETY: port 0xE9 = QEMU ISA debug device, no memory effect.
        // `::core` because `core` is ambiguous here (crate::fs::exofs::core module).
        unsafe {
            ::core::arch::asm!("out 0xE9, al", in("al") b'#', options(nomem, nostack));
            ::core::arch::asm!("out 0xE9, al", in("al") tag, options(nomem, nostack));
        }
    }
    fsdbg(b'0');

    // Virtio block driver mounting phase
    crate::fs::exofs::storage::virtio_adapter::init_global_disk();
    fsdbg(b'1');

    // Ramdisk fallback (Multiboot2 exofs_root module) if virtio-blk absent (e.g. Live ISO)
    if !crate::fs::exofs::storage::virtio_adapter::has_global_disk() {
        fsdbg(b'M');
        if crate::fs::exofs::storage::ramdisk::init_global_disk_ramdisk() {
            fsdbg(b'm');
        }
    }

    // ATA/IDE fallback (PIO, 0x1F0 master) if virtio-blk and ramdisk absent — Bochs (which emulates
    // neither virtio nor AHCI/NVMe) or QEMU `pc` machine. Allows reading ExoFS rootfs
    // from a legacy IDE disk for diagnostic #25 under Bochs.
    if !crate::fs::exofs::storage::virtio_adapter::has_global_disk() {
        fsdbg(b'I');
        if crate::fs::exofs::storage::ata_pio::init_global_disk_ata() {
            fsdbg(b'i');
        }
    }

    // FIX-GPT: if disk is GPT-partitioned, locate the ExoFS ROOT partition
    // by its type-GUID and shift all ExoFS I/O to its starting LBA
    // (shared `exo-partition` parser). ADDENDUM: raw disk / legacy MBR / GPT
    // without ROOT partition -> no shift, superblock remains read at LBA 0
    // (behavior of current mkfs images, zero regression). Any parsing
    // error (CRC/signature) falls back to LBA 0.
    if let Some(rp) = crate::fs::exofs::storage::partition_scan::resolve_exofs_partition() {
        // ExoFS volume = ROOT partition: effective size (used by
        // boot recovery to bound its reads) becomes that of the partition.
        let bs = crate::fs::exofs::storage::virtio_adapter::current_global_disk()
            .map(|d| d.block_size() as u64)
            .unwrap_or(512);
        disk_size_bytes = rp.sectors.saturating_mul(bs);
    }
    fsdbg(b'g');

    register_storage_flush_barrier();
    fsdbg(b'2');

    // Phase 1: Boot recovery — selects valid Epoch
    boot_recovery_sequence(disk_size_bytes).map_err(|_| ExofsError::RecoveryFailed)?;
    fsdbg(b'3');

    // Phase 2: VFS registration (register_exofs_syscalls() — called via exofs_register_fs()).
    // Omitted here: registration performed after boot via exofs_register_fs().

    // Phase 3: POSIX compatibility layer initialization
    posix_bridge::posix_bridge_init()?;
    fsdbg(b'4');
    crate::process::lifecycle::exit::register_vfs_close_all_pid_hook(
        posix_bridge::vfs_close_all_pid,
    );
    crate::process::lifecycle::exec::register_close_exec_handles_hook(
        crate::syscall::fs_bridge::close_exec_handles_for_pid,
    );
    crate::process::core::pcb::file_table::register_fork_dup_handle_hook(
        crate::syscall::fs_bridge::dup_handle_for_fork,
    );
    crate::process::core::pcb::file_table::register_close_handle_hook(
        crate::syscall::fs_bridge::close_handle_on_exit,
    );

    // Phase 4: Background GC/writeback threads
    // GC kthread runs at low priority — it calls run_gc_two_phase() in a loop.
    // Scheduler preempts it between cycles.
    let gc_params = KthreadParams {
        name: "exofs-gc",
        entry: exofs_gc_kthread,
        arg: 0,
        target_cpu: 0,
        priority: Priority(130), // nice +10: background, but not starved behind idle.
    };
    // Ignore error if scheduler is not yet active at boot
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

/// ExoFS GC kthread entry function.
///
/// Executes in a loop at low priority level.
/// On each pass: reclaims orphan blobs at least 2 epochs old.
fn exofs_gc_kthread(_arg: usize) -> ! {
    // Background GC must never win the first userspace handoff. It only scans
    // after epochs have matured; before that it backs off without touching the
    // global blob cache.
    gc_backoff();

    loop {
        let current_epoch = crate::fs::exofs::syscall::epoch_commit::current_epoch();
        if current_epoch > 2 {
            // Runs a complete GC cycle (scan + collect) for epochs older than 2.
            let epoch_threshold = current_epoch - 2;
            let _ = crate::fs::exofs::syscall::gc_trigger::run_gc_two_phase(epoch_threshold);
        }

        if !crate::scheduler::timer::sleep_ns(EXOFS_WRITEBACK_INTERVAL_NS) {
            gc_backoff();
        }
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
    // FIX-EXOFS-CORE-1 (AUDIT-EXOFS §2): writeback no longer persists raw blobs
    // in isolation — it triggers a full transactional epoch commit
    // (flush of dirty blobs + journal + EpochRoot/Record + NVMe barriers), the only
    // guarantee of atomicity and recoverable state on reboot. `commit_current_
    // epoch` short-circuits cleanly if there is nothing to commit or if a concurrent
    // commit (fsync/sync) is already in progress.
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

/// Registers ExoFS in the kernel VFS table.
/// Returns pointer to VFS operations.
pub fn exofs_register_fs() -> Result<(), ExofsError> {
    posix_bridge::vfs_compat::register_exofs_vfs_ops()
}

/// Clean unmount: flush epoch, await GC, sync superblock.
pub fn exofs_shutdown() -> Result<(), ExofsError> {
    if !EXOFS_INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }

    // Commit current epoch (force=true, flush all pending blobs).
    // Uses internal do_shutdown_commit() API which does not go through userspace.
    let commit_args = EpochCommitArgs {
        flags: epoch_flags::FORCE,
        _pad: 0,
        epoch_id: 0, // 0 = current epoch
        checksum: 0, // no checksum verification at shutdown
        hints: 0,
    };
    // Ignore CommitInProgress error (a concurrent commit will finish the work).
    match do_shutdown_commit(&commit_args) {
        Ok(_) | Err(ExofsError::CommitInProgress) => {}
        Err(e) => return Err(e),
    }

    EXOFS_INITIALIZED.store(false, Ordering::Release);
    log_kernel!("[exofs] arrêt propre effectué");
    Ok(())
}
