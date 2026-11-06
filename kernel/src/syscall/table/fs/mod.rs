//! # syscall/table/fs/mod.rs — Handlers for I/O, file descriptors, VFS, and ExoFS
//!
//! This module groups cleanly and modularly all syscall handlers
//! related to the file system and streams:
//! - `exofs_wrappers` : ABI wrappers for native ExoFS system calls.
//! - `io`             : Simple, vectored (readv/writev), positioned (pread/pwrite), and stream (splice/tee/vmsplice/sendfile) read/write.
//! - `descriptors`    : File descriptor manipulation (open/close/lseek/dup/fcntl/ioctl/truncate/sync).
//! - `path_ops`       : Path traversal and manipulation (stat/access/mkdir/unlink/rename/link/symlink/chmod/chown/statx).
//! - `dirs`           : Directory navigation and process FS context (getdents64/getcwd/chdir/umask/sync).
//! - `poll_event`     : I/O multiplexing and events (pipe/eventfd/inotify/poll/select/epoll).
//! - `sockets`        : Standard BSD Sockets network interface (socket/connect/bind/listen/accept/sendto/recvfrom/etc.).
//! - `system_misc`    : Resource management and system reboot (getrlimit/setrlimit/reboot/mremap/msync/clock_gettime).

pub mod descriptors;
pub mod dirs;
pub mod exofs_wrappers;
pub mod io;
pub mod path_ops;
pub mod poll_event;
pub mod sockets;
pub mod system_misc;

pub use descriptors::*;
pub use dirs::*;
pub use exofs_wrappers::*;
pub use io::*;
pub use path_ops::*;
pub use poll_event::*;
pub use sockets::*;
pub use system_misc::*;
