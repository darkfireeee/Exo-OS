//! Translation Module
//!
//! Converts between POSIX and Exo-OS representations

pub mod errno;
pub mod fd_to_cap;
pub mod perms_to_rights;
pub mod signals_to_msgs;

pub use errno::{fs_error_to_errno, memory_error_to_errno, strerror, Errno};
pub use fd_to_cap::{fd_to_cap_id, fd_to_capability, validate_fd_capability};
pub use perms_to_rights::{default_dir_mode, default_file_mode, mode_to_rights, rights_to_mode};
pub use signals_to_msgs::{default_action, message_to_signal, signal_to_message, Signal};
