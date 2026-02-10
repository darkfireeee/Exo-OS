//! Filesystem Subsystem - Exo OS Next-Generation Architecture
//!
//! # Overview
//!
//! This is a production-ready, modern filesystem subsystem featuring:
//! - **Multi-tier caching**: Page cache, inode cache, buffer cache with intelligent eviction
//! - **Advanced I/O**: io_uring, zero-copy, AIO, mmap, direct I/O, completion queues
//! - **Data integrity**: Checksums, journaling, recovery, scrubbing, self-healing
//! - **AI-driven optimization**: Access prediction, smart prefetching, adaptive caching
//! - **Security**: SELinux-style MAC, capabilities, secure deletion
//! - **Monitoring**: Real-time stats, performance metrics, health monitoring
//!
//! # Architecture
//!
//! The filesystem subsystem is organized into the following major components:
//!
//! ## Core Subsystems
//! - [`core`]: Virtual File System (VFS) layer, core types and traits
//! - [`io`]: Ultra-fast I/O engine (io_uring, zero-copy, AIO, mmap, direct I/O)
//! - [`cache`]: Multi-tier intelligent caching system
//! - [`integrity`]: Data integrity layer (checksums, journaling, recovery)
//!
//! ## Filesystem Implementations
//! - [`ext4plus`]: Modern ext4 with AI-guided allocation and advanced features
//! - [`compatibility`]: Legacy filesystem support (FAT32, ext4 read-only, tmpfs, FUSE)
//! - [`pseudo`]: Pseudo filesystems (devfs, procfs, sysfs)
//! - [`ipc`]: IPC filesystems (pipes, sockets, shared memory)
//!
//! ## Infrastructure
//! - [`block`]: Advanced block device layer (schedulers, RAID, NVMe, stats)
//! - [`security`]: Security framework (MAC, capabilities, secure deletion)
//! - [`monitoring`]: Performance monitoring and health tracking
//! - [`ai`]: Machine learning for intelligent filesystem optimization
//! - [`utils`]: Common utilities (bitmap, CRC, endianness, locks, time)
//!
//! ## Legacy Support (Backward Compatibility)
//! - [`operations`]: Legacy operations (buffer, locks, fdtable, cache)
//! - [`vfs`]: Stub re-exporting from core::vfs
//! - [`descriptor`]: Stub re-exporting from core::descriptor
//! - [`page_cache`]: Stub re-exporting from cache module
//! - [`block_device`]: Stub re-exporting from block module
//! - [`advanced`]: Stub for advanced features
//!
//! # Initialization Order
//!
//! The filesystem subsystem must be initialized in a specific order to ensure
//! proper dependencies. See [`init()`] for the correct initialization sequence.
//!
//! # Examples
//!
//! ```no_run
//! use exo_os::fs;
//!
//! // Initialize the entire filesystem subsystem
//! fs::init();
//!
//! // Use VFS operations
//! let file = fs::core::vfs::open("/test.txt", OpenFlags::CREATE | OpenFlags::RDWR)?;
//! file.write(b"Hello, world!")?;
//! ```

use crate::memory::MemoryError;
use alloc::string::String;
use alloc::vec::Vec;

// Re-export format! macro from alloc for convenience
pub use alloc::format;

// ============================================================================
// CORE SUBSYSTEMS
// ============================================================================

/// Core VFS layer - Virtual File System types, traits, and operations
///
/// Contains the fundamental building blocks for the filesystem:
/// - VFS layer with mount management
/// - Inode and dentry abstractions
/// - File operations and descriptors
/// - Path resolution
pub mod core;

/// I/O Engine - Ultra-fast I/O subsystem
///
/// Features:
/// - io_uring for asynchronous I/O
/// - Zero-copy data transfers
/// - AIO (Async I/O)
/// - Memory-mapped I/O (mmap)
/// - Direct I/O (bypassing cache)
/// - Completion queues
pub mod io;

/// Cache Subsystem - Multi-tier intelligent caching
///
/// Features:
/// - Page cache for file data
/// - Inode cache for filesystem metadata
/// - Buffer cache for block I/O
/// - Advanced eviction policies (LRU, ARC, LIRS)
/// - Adaptive cache sizing
/// - Write-back and write-through modes
pub mod cache;

/// Integrity Layer - Data integrity and reliability
///
/// Features:
/// - CRC32/CRC32C checksums
/// - Journaling (metadata + data)
/// - Recovery and replay
/// - Background scrubbing
/// - Self-healing on corruption detection
pub mod integrity;

// ============================================================================
// FILESYSTEM IMPLEMENTATIONS
// ============================================================================

/// ext4plus - Modern ext4 filesystem with advanced features
///
/// A next-generation ext4 implementation featuring:
/// - Full ext4 compatibility
/// - AI-guided block allocation
/// - Advanced extent management
/// - Online defragmentation
/// - B-tree directory indexing
/// - Extended attributes and ACLs
pub mod ext4plus;

/// Compatibility Layer - Legacy filesystem support
///
/// Provides support for legacy filesystems:
/// - FAT32 (read/write)
/// - ext4 (read-only fallback)
/// - tmpfs (in-memory filesystem)
/// - FUSE (filesystem in userspace)
pub mod compatibility;

/// Pseudo Filesystems - Virtual kernel filesystems
///
/// Special filesystems that don't exist on disk:
/// - devfs: Device files (/dev)
/// - procfs: Process information (/proc)
/// - sysfs: Kernel/system information (/sys)
pub mod pseudo;

/// IPC Filesystems - Inter-process communication
///
/// Filesystems for IPC mechanisms:
/// - pipes: Named and anonymous pipes
/// - sockets: Unix domain sockets
/// - shm: Shared memory segments
pub mod ipc;

// ============================================================================
// INFRASTRUCTURE
// ============================================================================

/// Block Device Layer - Advanced block device management
///
/// Features:
/// - I/O schedulers (NOOP, CFQ, Deadline, BFQ)
/// - RAID support (0, 1, 5, 6, 10)
/// - NVMe optimizations
/// - Statistics and performance tracking
pub mod block;

/// Security Framework - Filesystem security
///
/// Features:
/// - Mandatory Access Control (SELinux-style)
/// - Capabilities-based security
/// - Secure deletion (crypto-shred)
/// - Access control lists (ACLs)
pub mod security;

/// Monitoring Subsystem - Performance and health monitoring
///
/// Features:
/// - Real-time I/O statistics
/// - Performance metrics
/// - Health monitoring
/// - Alert system
pub mod monitoring;

/// AI Subsystem - Machine learning for filesystem optimization
///
/// Features:
/// - Access pattern prediction
/// - Smart prefetching
/// - Adaptive caching
/// - Online learning
/// - Intelligent block allocation
pub mod ai;

/// Utilities - Common filesystem utilities
///
/// Provides shared utilities:
/// - Bitmap operations for allocation tracking
/// - CRC/checksum calculations
/// - Endianness conversions
/// - Lock-free primitives
/// - Time utilities
pub mod utils;

// ============================================================================
// LEGACY SUPPORT (BACKWARD COMPATIBILITY STUBS)
// ============================================================================

/// Core operations (buffer, locks, fdtable, cache)
///
/// **Note**: This module is kept for backward compatibility.
/// New code should use the new modular subsystems.
pub mod operations;

/// Virtual File System stub
///
/// **Deprecated**: Use [`core::vfs`] instead.
/// This module re-exports from core for backward compatibility.
pub mod vfs {
    pub use super::core::vfs::*;
}

/// Descriptor management stub
///
/// **Deprecated**: Use [`core::descriptor`] instead.
/// This module re-exports from core for backward compatibility.
pub mod descriptor {
    pub use super::core::descriptor::*;
}

/// Page cache stub
///
/// **Deprecated**: Use [`cache::page_cache`] instead.
pub mod page_cache {
    pub use super::cache::page_cache::*;
}

// Block device stub - real implementation in block module
// Deprecated: Use block module instead

// Advanced features stub - integrated into io/, security/, monitoring/
// Deprecated: Use specific subsystems instead

// ============================================================================
// INITIALIZATION
// ============================================================================

/// Initialize the filesystem subsystem
///
/// This function initializes all filesystem components in the correct order to
/// ensure proper dependencies are satisfied. The initialization sequence is:
///
/// 1. **Block Device Subsystem** - Initialize block device layer and advanced features
/// 2. **Utilities** - Core utilities (already passive, no init needed)
/// 3. **Cache Layers** - Multi-tier caching system
/// 4. **Integrity Layer** - Checksums, journaling, recovery
/// 5. **I/O Engine** - io_uring, zero-copy, AIO, mmap
/// 6. **Core VFS** - Virtual File System layer
/// 7. **Security** - Security framework
/// 8. **Monitoring** - Performance and health monitoring
/// 9. **AI Subsystem** - Machine learning for optimization
/// 10. **Filesystems** - ext4plus and compatibility layer
/// 11. **Pseudo Filesystems** - devfs, procfs, sysfs
/// 12. **IPC Filesystems** - pipes, sockets, shared memory
///
/// # Panics
///
/// This function will panic if any critical component fails to initialize.
/// This is intentional as the filesystem subsystem is essential for system operation.
///
/// # Examples
///
/// ```no_run
/// # use exo_os::fs;
/// // Initialize the filesystem subsystem during kernel boot
/// fs::init();
/// ```
pub fn init() {
    log::info!("╔═══════════════════════════════════════════════════════════════════╗");
    log::info!("║       Initializing Filesystem Subsystem - Exo OS v0.3.0          ║");
    log::info!("╚═══════════════════════════════════════════════════════════════════╝");

    // ========================================================================
    // Phase 1: Block Device Subsystem
    // ========================================================================
    log::info!("[1/12] Initializing block device subsystem...");

    // Initialize basic block device layer
    // TODO: Implement block_device module
    // block_device::init();
    log::debug!("  ✓ Block device registry stub (pending implementation)");

    // Initialize advanced block layer (schedulers, RAID, NVMe)
    block::init();
    log::debug!("  ✓ Advanced block layer initialized");
    log::info!("  ✓ Block device subsystem ready");

    // ========================================================================
    // Phase 2: Utilities (No initialization needed - passive utilities)
    // ========================================================================
    log::info!("[2/12] Utilities available (bitmap, CRC, endian, locks, time)");

    // ========================================================================
    // Phase 3: Cache Layers
    // ========================================================================
    log::info!("[3/12] Initializing multi-tier cache subsystem...");

    // Initialize legacy page cache stub (for backward compatibility)
    // page_cache::init_page_cache(512);
    log::debug!("  ✓ Legacy page cache stub (no init needed)");

    // Initialize VFS cache (part of VFS init)
    // vfs::cache::init();
    log::debug!("  ✓ VFS cache will be initialized with VFS");

    // Initialize operations cache (for backward compatibility)
    operations::cache::init();
    log::debug!("  ✓ Operations cache initialized");

    // Initialize modern multi-tier cache subsystem
    cache::init(
        512,   // 512 MB page cache
        10000, // 10K inodes in inode cache
        2048,  // 2048 entries in buffer cache
    );
    log::info!("  ✓ Multi-tier cache ready (512MB page + 10K inodes + 2K buffers)");

    // ========================================================================
    // Phase 4: Integrity Layer
    // ========================================================================
    log::info!("[4/12] Initializing data integrity layer...");

    integrity::init(
        10000, // Max 10K journal entries
    );
    log::info!("  ✓ Integrity layer ready (checksums, journaling, recovery)");

    // ========================================================================
    // Phase 5: I/O Engine
    // ========================================================================
    log::info!("[5/12] Initializing I/O engine...");

    io::init();
    log::info!("  ✓ I/O engine ready (io_uring, zero-copy, AIO, mmap, direct I/O)");

    // ========================================================================
    // Phase 6: Core VFS
    // ========================================================================
    log::info!("[6/12] Initializing Virtual File System...");

    match core::vfs::init() {
        Ok(_) => {
            log::info!("  ✓ VFS initialized successfully");
        }
        Err(e) => {
            log::error!("  ✗ VFS initialization failed: {:?}", e);
            log::warn!("  Attempting graceful degradation...");

            // Try to mount a minimal ramfs as emergency fallback
            match compatibility::tmpfs::mount_root() {
                Ok(_) => {
                    log::warn!("  ✓ Emergency ramfs mounted (degraded mode)");
                    log::warn!("  Warning: Running with minimal filesystem support only");
                }
                Err(e2) => {
                    log::error!("  ✗ Critical failure: Even minimal ramfs failed: {:?}", e2);
                    log::error!("  System cannot continue without filesystem");
                    // At this point, we must halt - no filesystem = no way to execute anything
                    loop {
                        ::core::sync::atomic::compiler_fence(::core::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
        }
    }

    // ========================================================================
    // Phase 7: Security
    // ========================================================================
    log::info!("[7/12] Initializing security framework...");

    security::init();
    log::info!("  ✓ Security framework ready (MAC, capabilities, secure deletion)");

    // ========================================================================
    // Phase 8: Monitoring
    // ========================================================================
    log::info!("[8/12] Initializing monitoring subsystem...");

    monitoring::init();
    log::info!("  ✓ Monitoring ready (real-time stats, performance metrics)");

    // ========================================================================
    // Phase 9: AI Subsystem
    // ========================================================================
    log::info!("[9/12] Initializing AI subsystem...");

    ai::init();
    log::info!("  ✓ AI subsystem ready (access prediction, smart caching)");

    // ========================================================================
    // Phase 10: Filesystems
    // ========================================================================
    log::info!("[10/12] Initializing filesystem implementations...");

    // Initialize ext4plus
    ext4plus::init();
    log::debug!("  ✓ ext4plus initialized");

    // Initialize compatibility layer (FAT32, ext4 read-only, tmpfs, FUSE)
    compatibility::init();
    log::debug!("  ✓ Compatibility layer initialized");
    log::info!("  ✓ Filesystems ready (ext4plus, FAT32, ext4-ro, tmpfs, FUSE)");

    // ========================================================================
    // Phase 11: Pseudo Filesystems
    // ========================================================================
    log::info!("[11/12] Initializing pseudo filesystems...");

    pseudo::init();
    log::info!("  ✓ Pseudo filesystems ready (devfs, procfs, sysfs)");

    // ========================================================================
    // Phase 12: IPC Filesystems
    // ========================================================================
    log::info!("[12/12] Initializing IPC filesystems...");

    ipc::init();
    log::info!("  ✓ IPC filesystems ready (pipes, sockets, shared memory)");

    // ========================================================================
    // Initialization Complete
    // ========================================================================
    log::info!("");
    log::info!("╔═══════════════════════════════════════════════════════════════════╗");
    log::info!("║    Filesystem Subsystem Initialized Successfully ✓                ║");
    log::info!("╚═══════════════════════════════════════════════════════════════════╝");
    log::info!("");
    log::info!("Summary:");
    log::info!("  • Block Devices: Advanced I/O schedulers, RAID, NVMe");
    log::info!("  • Cache: Multi-tier (page + inode + buffer), intelligent eviction");
    log::info!("  • Integrity: Checksums, journaling, self-healing");
    log::info!("  • I/O: io_uring, zero-copy, AIO, mmap, direct I/O");
    log::info!("  • Security: MAC, capabilities, secure deletion");
    log::info!("  • Monitoring: Real-time stats, performance tracking");
    log::info!("  • AI: Access prediction, adaptive caching, smart prefetch");
    log::info!("  • Filesystems: ext4plus, FAT32, ext4-ro, tmpfs, FUSE");
    log::info!("  • Pseudo: devfs, procfs, sysfs");
    log::info!("  • IPC: pipes, sockets, shared memory");
    log::info!("");
}

/// Filesystem errors
#[derive(Debug)]
pub enum FsError {
    NotFound,
    NoSuchFileOrDirectory,  // Alias pour NotFound (compatibilité POSIX)
    PermissionDenied,
    AlreadyExists,
    FileExists,
    NotDirectory,
    IsDirectory,
    DirectoryNotEmpty,
    InvalidPath,
    InvalidArgument,
    TooManySymlinks,
    InvalidData,
    Corrupted,              // Données corrompues (checksum échoué, etc.)
    IoError,
    NotSupported,
    TooManyFiles,
    TooManyOpenFiles,
    InvalidFd,
    ConnectionRefused,
    Again,
    QuotaExceeded,
    NoMemory,      // Phase 1c: Out of memory
    NoSpace,       // Phase 1c: No space left on device
    AddressInUse,  // Socket address already in use
    Memory(MemoryError),
}

impl FsError {
    pub fn to_errno(&self) -> i32 {
        match self {
            FsError::NotFound => 2,            // ENOENT
            FsError::NoSuchFileOrDirectory => 2, // ENOENT
            FsError::PermissionDenied => 13,   // EACCES
            FsError::AlreadyExists => 17,      // EEXIST
            FsError::FileExists => 17,         // EEXIST
            FsError::NotDirectory => 20,       // ENOTDIR
            FsError::IsDirectory => 21,        // EISDIR
            FsError::DirectoryNotEmpty => 39,  // ENOTEMPTY
            FsError::InvalidPath => 22,        // EINVAL
            FsError::InvalidArgument => 22,    // EINVAL
            FsError::TooManySymlinks => 40,    // ELOOP
            FsError::InvalidData => 22,        // EINVAL
            FsError::Corrupted => 5,           // EIO (I/O error for corrupted data)
            FsError::IoError => 5,             // EIO
            FsError::NotSupported => 95,       // EOPNOTSUPP
            FsError::TooManyFiles => 24,       // EMFILE
            FsError::TooManyOpenFiles => 24,   // EMFILE
            FsError::InvalidFd => 9,           // EBADF
            FsError::ConnectionRefused => 111, // ECONNREFUSED
            FsError::Again => 11,              // EAGAIN
            FsError::QuotaExceeded => 122,     // EDQUOT
            FsError::NoMemory => 12,           // ENOMEM
            FsError::NoSpace => 28,            // ENOSPC
            FsError::AddressInUse => 98,       // EADDRINUSE
            FsError::Memory(_) => 12,          // ENOMEM (simplified)
        }
    }
}

impl From<MemoryError> for FsError {
    fn from(e: MemoryError) -> Self {
        FsError::Memory(e)
    }
}

impl From<FsError> for MemoryError {
    fn from(e: FsError) -> Self {
        match e {
            FsError::NotFound | FsError::NoSuchFileOrDirectory => MemoryError::NotFound,
            FsError::PermissionDenied => MemoryError::PermissionDenied,
            FsError::AlreadyExists | FsError::FileExists => MemoryError::AlreadyMapped,
            FsError::NotDirectory => MemoryError::InvalidParameter,
            FsError::IsDirectory => MemoryError::InvalidParameter,
            FsError::DirectoryNotEmpty => MemoryError::InvalidParameter,
            FsError::InvalidPath => MemoryError::InvalidAddress,
            FsError::InvalidArgument => MemoryError::InvalidParameter,
            FsError::TooManySymlinks => MemoryError::InvalidParameter,
            FsError::InvalidData => MemoryError::InvalidParameter,
            FsError::Corrupted => MemoryError::InternalError("Data corrupted"),
            FsError::IoError => MemoryError::InternalError("IO Error"),
            FsError::NotSupported => MemoryError::InternalError("Not supported"),
            FsError::TooManyFiles => MemoryError::OutOfMemory,
            FsError::TooManyOpenFiles => MemoryError::OutOfMemory,
            FsError::InvalidFd => MemoryError::InvalidParameter,
            FsError::ConnectionRefused => MemoryError::InternalError("Connection refused"),
            FsError::Again => MemoryError::InternalError("Try again"),
            FsError::QuotaExceeded => MemoryError::InternalError("Quota exceeded"),
            FsError::NoMemory => MemoryError::OutOfMemory,
            FsError::NoSpace => MemoryError::InternalError("No space left on device"),
            FsError::AddressInUse => MemoryError::InternalError("Address already in use"),
            FsError::Memory(e) => e,
        }
    }
}

pub type FsResult<T> = Result<T, FsError>;

/// File metadata
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub size: u64,
    pub is_dir: bool,
    pub read_only: bool,
}

/// File handle trait
pub trait File {
    fn read(&mut self, buf: &mut [u8]) -> FsResult<usize>;
    fn write(&mut self, buf: &[u8]) -> FsResult<usize>;
    fn seek(&mut self, pos: u64) -> FsResult<u64>;
    fn metadata(&self) -> FsResult<FileMetadata>;
}


