//! # syscall/fs_bridge/special — Special & pseudo-filesystem descriptors
//!
//! Segregates TTY IPC, pseudo-blob lifecycle, `eventfd`, anonymous pipes,
//! zero-copy `splice`/`tee`/`vmsplice`, and `AF_UNIX` stream/datagram/`SCM_RIGHTS` sockets.

pub mod eventfd;
pub mod pipe;
pub mod pseudo_blob;
pub mod splice;
pub mod tty_bridge;
pub mod unix_socket;

pub use eventfd::*;
pub use pipe::*;
pub use pseudo_blob::*;
pub use splice::*;
pub use tty_bridge::*;
pub use unix_socket::*;
