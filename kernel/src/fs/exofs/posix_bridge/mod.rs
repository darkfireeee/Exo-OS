//! mod.rs — POSIX bridge for ExoFS (posix_bridge)
//!
//! Aggregates the four POSIX bridge submodules:
//!   - `fcntl_lock`     : POSIX byte-range locks (F_SETLK / F_GETLK)
//!   - `inode_emulation`: bidirectional mapping object_id <-> ino_t
//!   - `mmap`           : memory mapping management (mmap/munmap/msync/mprotect)
//!   - `vfs_compat`     : VFS layer (lookup/open/read/write/getattr/readdir/...)
//!
//! Also provides `posix_bridge_init()`, `posix_bridge_shutdown()`, and
//! `posix_bridge_stats()` for initialization, teardown, and diagnostics.
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusively.

pub mod fcntl_lock;
pub mod inode_emulation;
pub mod mmap;
pub mod vfs_compat;

// ─────────────────────────────────────────────────────────────────────────────
// Ergonomic re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use fcntl_lock::{
    ByteRangeLock, FcntlCmd, LockInfo, LockKind, FCNTL_LOCK_TABLE, MAX_LOCKS_PER_OBJECT,
    MAX_OBJECTS_LOCKED,
};

pub use inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
    INO_MAX_CACHE, INO_RESERVED, INO_ROOT,
};

pub use mmap::{
    align_up, map_flags, map_prot, pages_for, ranges_overlap, validate_page_aligned, MappingState,
    MmapEntry, MMAP_MAX_MAPPINGS, MMAP_PAGE_SIZE, MMAP_TABLE,
};

pub use vfs_compat::{
    file_mode, open_flags, register_exofs_vfs_ops, root_inode, vfs_close, vfs_close_all_pid,
    vfs_create, vfs_getattr, vfs_is_registered, vfs_lookup, vfs_mkdir, vfs_open, vfs_open_count,
    vfs_read, vfs_readdir, vfs_rename, vfs_rmdir, vfs_symlink, vfs_truncate, vfs_unlink, vfs_write,
    VfsDirent, VfsFd, VfsInode, VFS_NAME_MAX, VFS_ROOT_INO,
};

use crate::fs::exofs::core::ExofsResult;

// ─────────────────────────────────────────────────────────────────────────────
// Global bridge state
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static BRIDGE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static BRIDGE_INIT_COUNT: AtomicU64 = AtomicU64::new(0);
static BRIDGE_SHUTDOWN_COUNT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

/// POSIX bridge metrics snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PosixBridgeStats {
    /// Number of active locks (across all tables).
    pub lock_count: usize,
    /// Number of distinct locked objects.
    pub locked_objects: usize,
    /// Number of cached inodes.
    pub inode_count: usize,
    /// Next ino to be allocated.
    pub next_ino: ObjectIno,
    /// Number of active memory mappings.
    pub mmap_count: usize,
    /// Total number of mapped bytes.
    pub mmap_total_bytes: u64,
    /// Number of open file descriptors.
    pub open_fd_count: usize,
    /// Is the ExoFS VFS registered?
    pub vfs_registered: bool,
    /// Has the bridge been initialized?
    pub initialized: bool,
    /// Number of initializations since boot.
    pub init_count: u64,
    /// Number of shutdowns since boot.
    pub shutdown_count: u64,
}

/// Collects statistics across all submodules.
pub fn posix_bridge_stats() -> PosixBridgeStats {
    PosixBridgeStats {
        lock_count: FCNTL_LOCK_TABLE.total_lock_count(),
        locked_objects: FCNTL_LOCK_TABLE.locked_object_count(),
        inode_count: INODE_EMULATION.count(),
        next_ino: INODE_EMULATION.peek_next_ino(),
        mmap_count: MMAP_TABLE.mapping_count(),
        mmap_total_bytes: MMAP_TABLE.total_mapped_bytes(),
        open_fd_count: vfs_open_count(),
        vfs_registered: vfs_is_registered(),
        initialized: BRIDGE_INITIALIZED.load(Ordering::Acquire),
        init_count: BRIDGE_INIT_COUNT.load(Ordering::Relaxed),
        shutdown_count: BRIDGE_SHUTDOWN_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the entire POSIX bridge.
///
/// - Ensures the root inode is present.
/// - Registers ExoFS as a VFS operator (no-op if already registered).
/// - Idempotent: multiple invocations are safe.
pub fn posix_bridge_init() -> ExofsResult<()> {
    if BRIDGE_INITIALIZED
        .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
        .is_err()
    {
        // Already initialized — increment counter without returning an error.
        BRIDGE_INIT_COUNT.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    // Ensure the root inode is present.
    INODE_EMULATION.ensure_root()?;
    // Register VFS ops.
    let _ = register_exofs_vfs_ops(); // May return ObjectAlreadyExists if already registered — ignored.
    BRIDGE_INIT_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Shuts down the POSIX bridge: releases all resources.
///
/// Must be called during filesystem unmount.
pub fn posix_bridge_shutdown() {
    // Release tables in reverse order of initialization.
    MMAP_TABLE.clear();
    FCNTL_LOCK_TABLE.clear();
    INODE_EMULATION.clear();
    BRIDGE_INITIALIZED.store(false, Ordering::Release);
    BRIDGE_SHUTDOWN_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Releases all resources belonging to a process (on exit/kill).
///
/// - Removes all mappings for pid.
/// - Removes all locks for pid.
/// - Closes all fds for pid.
pub fn posix_bridge_process_exit(pid: u32) {
    MMAP_TABLE.munmap_all_pid(pid);
    FCNTL_LOCK_TABLE.release_all_pid(pid as u64);
    vfs_close_all_pid(pid);
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-cutting validations
// ─────────────────────────────────────────────────────────────────────────────

/// Verifies internal bridge consistency (sanity assertions).
/// Returns the number of detected anomalies.
/// RECUR-01: while.
pub fn posix_bridge_check() -> usize {
    let mut anomalies = 0usize;
    // Verify that the root inode is registered if the bridge is initialized.
    if BRIDGE_INITIALIZED.load(Ordering::Acquire) {
        if !INODE_EMULATION.contains_ino(INO_ROOT) {
            anomalies = anomalies.wrapping_add(1);
        }
        if !vfs_is_registered() {
            anomalies = anomalies.wrapping_add(1);
        }
    }
    anomalies
}

/// Returns the total number of active locks in the global table.
/// RECUR-01 traversal: while.
pub fn total_lock_count() -> usize {
    FCNTL_LOCK_TABLE.total_lock_count()
}

// ─────────────────────────────────────────────────────────────────────────────
// Version constants
// ─────────────────────────────────────────────────────────────────────────────

pub const POSIX_BRIDGE_MAJOR: u8 = 1;
pub const POSIX_BRIDGE_MINOR: u8 = 0;
pub const POSIX_BRIDGE_PATCH: u8 = 0;
pub const POSIX_BRIDGE_MAGIC: u32 = 0x5042_5247; // "PBRG"

/// Returns the bridge version encoded as a u32.
/// ARITH-02: wrapping_shl.
pub fn posix_bridge_version() -> u32 {
    ((POSIX_BRIDGE_MAJOR as u32).wrapping_shl(16))
        | ((POSIX_BRIDGE_MINOR as u32).wrapping_shl(8))
        | (POSIX_BRIDGE_PATCH as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_stats_type() {
        let s = PosixBridgeStats::default();
        assert_eq!(s.lock_count, 0);
        assert!(!s.initialized);
    }

    #[test]
    fn test_bridge_version() {
        let v = posix_bridge_version();
        assert_eq!(v >> 16, POSIX_BRIDGE_MAJOR as u32);
        assert_eq!((v >> 8) & 0xFF, POSIX_BRIDGE_MINOR as u32);
        assert_eq!(v & 0xFF, POSIX_BRIDGE_PATCH as u32);
    }

    #[test]
    fn test_bridge_magic() {
        assert_eq!(POSIX_BRIDGE_MAGIC, 0x5042_5247);
    }

    #[test]
    fn test_bridge_init_idempotent() {
        let b = AtomicBool::new(false);
        // Verify that compare_exchange returns Err if already true.
        b.store(true, Ordering::Relaxed);
        assert!(b
            .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
            .is_err());
    }

    #[test]
    fn test_align_up_reexport() {
        // align_up is re-exported from mmap.
        assert_eq!(align_up(1, 4096), 4096);
    }

    #[test]
    fn test_pages_for_reexport() {
        assert_eq!(pages_for(4097), 2);
    }

    #[test]
    fn test_ranges_overlap_reexport() {
        assert!(ranges_overlap(0, 10, 5, 10));
        assert!(!ranges_overlap(0, 5, 5, 5));
    }

    #[test]
    fn test_vfs_root_ino() {
        assert_eq!(root_inode(), 1);
        assert_eq!(VFS_ROOT_INO, 1);
    }

    #[test]
    fn test_ino_root_constant() {
        assert_eq!(INO_ROOT, 1);
        assert_eq!(INO_RESERVED, 2);
    }

    #[test]
    fn test_mmap_page_size() {
        assert_eq!(MMAP_PAGE_SIZE, 4096);
    }

    #[test]
    fn test_validate_page_aligned_reexport() {
        assert!(validate_page_aligned(0).is_ok());
        assert!(validate_page_aligned(1).is_err());
    }

    #[test]
    fn test_inode_flags_values() {
        assert_eq!(inode_flags::DIRECTORY, 0x0001);
        assert_eq!(inode_flags::SYMLINK, 0x0002);
        assert_eq!(inode_flags::REGULAR, 0x0004);
    }

    #[test]
    fn test_file_mode_ifdir() {
        assert_eq!(file_mode::S_IFDIR, 0o040000);
    }

    #[test]
    fn test_open_flags_rdonly() {
        assert_eq!(open_flags::O_RDONLY, 0);
        assert_ne!(open_flags::O_WRONLY, 0);
    }

    #[test]
    fn test_posix_bridge_stats_size() {
        // Sanity: structure must have a reasonable size.
        let sz = core::mem::size_of::<PosixBridgeStats>();
        assert!(sz > 0 && sz < 256);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bridge configuration
// ─────────────────────────────────────────────────────────────────────────────

/// POSIX bridge configuration parameters.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PosixBridgeConfig {
    /// Maximum number of locks per object (default: MAX_LOCKS_PER_OBJECT).
    pub max_locks_per_object: usize,
    /// Maximum number of concurrently locked objects.
    pub max_locked_objects: usize,
    /// Maximum inode cache size.
    pub max_inode_cache: usize,
    /// Maximum number of active memory mappings.
    pub max_mmap_entries: usize,
    /// Maximum number of open fds.
    pub max_open_fds: usize,
    /// Page size (must be a power of two).
    pub page_size: u64,
    /// Configuration version.
    pub version: u8,
}

impl Default for PosixBridgeConfig {
    fn default() -> Self {
        Self {
            max_locks_per_object: MAX_LOCKS_PER_OBJECT,
            max_locked_objects: MAX_OBJECTS_LOCKED,
            max_inode_cache: INO_MAX_CACHE,
            max_mmap_entries: MMAP_MAX_MAPPINGS,
            max_open_fds: vfs_compat::VFS_OPEN_MAX,
            page_size: MMAP_PAGE_SIZE,
            version: POSIX_BRIDGE_MAJOR,
        }
    }
}

impl PosixBridgeConfig {
    /// Validates configuration. Returns the number of errors.
    /// RECUR-01: no loop here (independent checks).
    pub fn validate(&self) -> usize {
        let mut errors = 0usize;
        if self.max_locks_per_object == 0 {
            errors = errors.wrapping_add(1);
        }
        if self.max_locked_objects == 0 {
            errors = errors.wrapping_add(1);
        }
        if self.max_inode_cache == 0 {
            errors = errors.wrapping_add(1);
        }
        if self.max_mmap_entries == 0 {
            errors = errors.wrapping_add(1);
        }
        if self.max_open_fds == 0 {
            errors = errors.wrapping_add(1);
        }
        if !self.page_size.is_power_of_two() {
            errors = errors.wrapping_add(1);
        }
        errors
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatted diagnostics
// ─────────────────────────────────────────────────────────────────────────────

/// Compact diagnostic report (serializable into log area).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PosixBridgeDiag {
    pub magic: u32,
    pub version: u32,
    pub stats: PosixBridgeStats,
    pub config_errors: usize,
    pub check_fail: usize,
}

/// Builds a complete diagnostic report.
pub fn posix_bridge_diag() -> PosixBridgeDiag {
    let cfg = PosixBridgeConfig::default();
    PosixBridgeDiag {
        magic: POSIX_BRIDGE_MAGIC,
        version: posix_bridge_version(),
        stats: posix_bridge_stats(),
        config_errors: cfg.validate(),
        check_fail: posix_bridge_check(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Name encoding/decoding utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Copies `name` into a fixed-size buffer N. Returns copied length.
/// OOM-02 n/a (static array). ARITH-02: min().
pub fn copy_name_to_buf<const N: usize>(name: &[u8], out: &mut [u8; N]) -> usize {
    let len = name.len().min(N).min(VFS_NAME_MAX);
    let mut i = 0usize;
    while i < len {
        out[i] = name[i];
        i = i.wrapping_add(1);
    }
    len
}

/// Compares two names in constant time (prevents timing oracles).
/// ARITH-02: wrapping additions.
pub fn names_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    let mut i = 0usize;
    while i < a.len() {
        diff |= a[i] ^ b[i];
        i = i.wrapping_add(1);
    }
    diff == 0
}

/// Returns length of a null-terminated name in a fixed-size buffer.
/// RECUR-01: while.
pub fn strnlen(buf: &[u8], max: usize) -> usize {
    let limit = max.min(buf.len());
    let mut i = 0usize;
    while i < limit && buf[i] != 0 {
        i = i.wrapping_add(1);
    }
    i
}

// ─────────────────────────────────────────────────────────────────────────────
// Atomic helper operations
// ─────────────────────────────────────────────────────────────────────────────

/// Increments an atomic counter saturated at `max`.
/// ARITH-02: saturating_add.
pub fn atomic_saturating_inc(counter: &core::sync::atomic::AtomicU64, max: u64) {
    let mut cur = counter.load(Ordering::Relaxed);
    while cur < max {
        match counter.compare_exchange_weak(
            cur,
            cur.saturating_add(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

/// Decrements an atomic counter floored at 0.
/// ARITH-02: saturating_sub.
pub fn atomic_saturating_dec(counter: &core::sync::atomic::AtomicU64) {
    let mut cur = counter.load(Ordering::Relaxed);
    while cur > 0 {
        match counter.compare_exchange_weak(
            cur,
            cur.saturating_sub(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mode/flags conversion functions
// ─────────────────────────────────────────────────────────────────────────────

/// Converts a POSIX mode into ExoFS inode flags.
pub fn mode_to_inode_flags(mode: u32) -> u32 {
    let ty = mode & file_mode::S_IFMT;
    if ty == file_mode::S_IFDIR {
        return inode_flags::DIRECTORY;
    }
    if ty == file_mode::S_IFLNK {
        return inode_flags::SYMLINK;
    }
    inode_flags::REGULAR
}

/// Converts ExoFS inode flags into POSIX mode type bits.
pub fn inode_flags_to_mode_type(flags: u32) -> u32 {
    if flags & inode_flags::DIRECTORY != 0 {
        return file_mode::S_IFDIR;
    }
    if flags & inode_flags::SYMLINK != 0 {
        return file_mode::S_IFLNK;
    }
    file_mode::S_IFREG
}

/// Returns true if open_flags require write access.
pub fn flags_require_write(flags: u32) -> bool {
    flags & open_flags::O_WRONLY != 0
        || flags & open_flags::O_RDWR != 0
        || flags & open_flags::O_TRUNC != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Additional tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_config_default_valid() {
        let cfg = PosixBridgeConfig::default();
        assert_eq!(cfg.validate(), 0);
    }

    #[test]
    fn test_config_invalid_page_size() {
        let mut cfg = PosixBridgeConfig::default();
        cfg.page_size = 3000; // Not a power of two.
        assert!(cfg.validate() > 0);
    }

    #[test]
    fn test_config_zero_locks() {
        let mut cfg = PosixBridgeConfig::default();
        cfg.max_locks_per_object = 0;
        assert!(cfg.validate() > 0);
    }

    #[test]
    fn test_diag_magic() {
        let d = posix_bridge_diag();
        assert_eq!(d.magic, POSIX_BRIDGE_MAGIC);
    }

    #[test]
    fn test_copy_name_to_buf() {
        let src = b"hello";
        let mut buf = [0u8; 16];
        let len = copy_name_to_buf(src, &mut buf);
        assert_eq!(len, 5);
        assert_eq!(&buf[..5], b"hello");
    }

    #[test]
    fn test_copy_name_truncation() {
        let src = b"toolongname";
        let mut buf = [0u8; 4];
        let len = copy_name_to_buf(src, &mut buf);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_names_equal() {
        assert!(names_equal(b"abc", b"abc"));
        assert!(!names_equal(b"abc", b"abd"));
        assert!(!names_equal(b"ab", b"abc"));
    }

    #[test]
    fn test_strnlen() {
        let buf = b"hello\x00world";
        assert_eq!(strnlen(buf, 64), 5);
        assert_eq!(strnlen(b"nul", 64), 3);
        assert_eq!(strnlen(b"", 64), 0);
    }

    #[test]
    fn test_mode_to_inode_flags() {
        assert_eq!(
            mode_to_inode_flags(file_mode::S_IFDIR),
            inode_flags::DIRECTORY
        );
        assert_eq!(
            mode_to_inode_flags(file_mode::S_IFLNK),
            inode_flags::SYMLINK
        );
        assert_eq!(
            mode_to_inode_flags(file_mode::S_IFREG),
            inode_flags::REGULAR
        );
    }

    #[test]
    fn test_inode_flags_to_mode_type() {
        assert_eq!(
            inode_flags_to_mode_type(inode_flags::DIRECTORY),
            file_mode::S_IFDIR
        );
        assert_eq!(
            inode_flags_to_mode_type(inode_flags::SYMLINK),
            file_mode::S_IFLNK
        );
        assert_eq!(
            inode_flags_to_mode_type(inode_flags::REGULAR),
            file_mode::S_IFREG
        );
    }

    #[test]
    fn test_flags_require_write() {
        assert!(flags_require_write(open_flags::O_WRONLY));
        assert!(flags_require_write(open_flags::O_RDWR));
        assert!(flags_require_write(open_flags::O_TRUNC));
        assert!(!flags_require_write(open_flags::O_RDONLY));
    }

    #[test]
    fn test_atomic_saturating_inc() {
        let c = core::sync::atomic::AtomicU64::new(u64::MAX - 1);
        atomic_saturating_inc(&c, u64::MAX);
        assert_eq!(c.load(Ordering::Relaxed), u64::MAX);
        atomic_saturating_inc(&c, u64::MAX); // does not exceed
        assert_eq!(c.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn test_atomic_saturating_dec() {
        let c = core::sync::atomic::AtomicU64::new(1);
        atomic_saturating_dec(&c);
        assert_eq!(c.load(Ordering::Relaxed), 0);
        atomic_saturating_dec(&c); // does not underflow below 0
        assert_eq!(c.load(Ordering::Relaxed), 0);
    }
}
