//! ExoFS lifecycle orchestration: mount, encrypted volume unlock, background
//! GC/writeback kthreads, VFS registration, and clean shutdown.
//!
//! Extracted from `exofs/mod.rs` so the module root stays a pure wiring facade.

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::error::ExofsError;
use crate::fs::exofs::platform::boot_trace_pair;
use crate::fs::exofs::posix_bridge;
use crate::fs::exofs::recovery::boot_recovery::boot_recovery_sequence;
use crate::fs::exofs::syscall::epoch_commit::{do_shutdown_commit, epoch_flags, EpochCommitArgs};
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::rust_core::sync::atomic::{AtomicBool, Ordering};
use crate::scheduler::core::task::Priority;

const EXOFS_WRITEBACK_INTERVAL_NS: u64 = 5_000_000_000;

/// Kernel log macro — no-op awaiting the Ring-0 logging system.
#[allow(unused_macros)]
macro_rules! log_kernel {
    ($($arg:tt)*) => {};
}

/// Global ExoFS module state — initialized once at boot.
static EXOFS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Unlocks the ExoFS volume if encrypted, installing the volume key derived
/// from `passphrase` via shared `exo-fscrypt`.
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

#[inline(always)]
fn fsdbg(tag: u8) {
    boot_trace_pair(b'#', tag);
}

/// Initializes ExoFS at kernel boot.
pub fn exofs_init(mut disk_size_bytes: u64) -> Result<(), ExofsError> {
    if EXOFS_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ExofsError::AlreadyMounted);
    }

    fsdbg(b'0');

    // Virtio block driver mounting phase (via fs/exofs_hal)
    crate::fs::exofs_hal::virtio_adapter::init_global_disk();
    fsdbg(b'1');

    // Ramdisk fallback (Multiboot2 exofs_root module) if virtio-blk absent
    if !crate::fs::exofs_hal::virtio_adapter::has_global_disk() {
        fsdbg(b'M');
        if crate::fs::exofs_hal::ramdisk::init_global_disk_ramdisk() {
            fsdbg(b'm');
        }
    }

    // ATA/IDE fallback (PIO, 0x1F0 master) if virtio-blk and ramdisk absent
    if !crate::fs::exofs_hal::virtio_adapter::has_global_disk() {
        fsdbg(b'I');
        if crate::fs::exofs_hal::ata_pio::init_global_disk_ata() {
            fsdbg(b'i');
        }
    }

    // Resolve GPT ExoFS ROOT partition if present
    if let Some(rp) = crate::fs::exofs::storage::partition_scan::resolve_exofs_partition() {
        let bs = crate::fs::exofs_hal::virtio_adapter::current_global_disk()
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

    // Phase 3: POSIX compatibility layer initialization & process hooks
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
    let gc_params = KthreadParams {
        name: "exofs-gc",
        entry: exofs_gc_kthread,
        arg: 0,
        target_cpu: 0,
        priority: Priority(130),
    };
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

fn exofs_gc_kthread(_arg: usize) -> ! {
    gc_backoff();

    loop {
        let current_epoch = crate::fs::exofs::syscall::epoch_commit::current_epoch();
        if current_epoch > 2 {
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
        crate::fs::exofs_hal::virtio_adapter::flush_global_disk,
    );
}

/// Registers ExoFS in the kernel VFS table.
pub fn exofs_register_fs() -> Result<(), ExofsError> {
    posix_bridge::vfs_compat::register_exofs_vfs_ops()
}

/// Clean unmount: flush epoch, await GC, sync superblock.
pub fn exofs_shutdown() -> Result<(), ExofsError> {
    if !EXOFS_INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }

    let commit_args = EpochCommitArgs {
        flags: epoch_flags::FORCE,
        _pad: 0,
        epoch_id: 0,
        checksum: 0,
        hints: 0,
    };
    match do_shutdown_commit(&commit_args) {
        Ok(_) | Err(ExofsError::CommitInProgress) => {}
        Err(e) => return Err(e),
    }

    EXOFS_INITIALIZED.store(false, Ordering::Release);
    log_kernel!("[exofs] arrêt propre effectué");
    Ok(())
}
