//! Advanced filesystem features module
//!
//! Contains advanced filesystem features:
//! - io_uring: Modern async I/O framework
//! - zero_copy: sendfile/splice/vmsplice zero-copy I/O
//! - aio: POSIX Async I/O (aio_read/aio_write)
//! - mmap: Memory-mapped files (mmap/munmap/msync/madvise)
//! - quota: Disk quota management (user/group/project)
//! - namespace: Mount namespace management for containers
//! - acl: POSIX.1e Access Control Lists
//! - notify: File change notifications (inotify/fanotify)

/// io_uring async I/O framework
pub mod io_uring;

/// Zero-copy I/O operations
pub mod zero_copy;

/// POSIX Async I/O
pub mod aio;

/// Memory-mapped files
pub mod mmap;

/// Disk quota management
pub mod quota;

/// Mount namespace management
pub mod namespace;

/// Access Control Lists
pub mod acl;

/// File change notifications
pub mod notify;
