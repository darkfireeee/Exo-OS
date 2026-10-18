//! # syscall/table/fs/mod.rs — Handlers pour I/O, descripteurs de fichiers, VFS et ExoFS
//!
//! Ce module regroupe de façon propre et modulaire l'ensemble des handlers de syscalls
//! liés au système de fichiers et aux flux :
//! - `exofs_wrappers` : Wrappers ABI pour les appels système natifs ExoFS.
//! - `io`             : Lecture/écriture simple, vectorielle (readv/writev), positionnée (pread/pwrite) et flux (splice/tee/vmsplice/sendfile).
//! - `descriptors`    : Manipulation des descripteurs de fichiers (open/close/lseek/dup/fcntl/ioctl/truncate/sync).
//! - `path_ops`       : Parcours et manipulation des chemins (stat/access/mkdir/unlink/rename/link/symlink/chmod/chown/statx).
//! - `dirs`           : Navigation répertoires et contexte FS du processus (getdents64/getcwd/chdir/umask/sync).
//! - `poll_event`     : Multiplexage et événements I/O (pipe/eventfd/inotify/poll/select/epoll).
//! - `sockets`        : Interface réseau standard BSD Sockets (socket/connect/bind/listen/accept/sendto/recvfrom/etc.).
//! - `system_misc`    : Gestion des ressources et reboot système (getrlimit/setrlimit/reboot/mremap/msync/clock_gettime).

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
