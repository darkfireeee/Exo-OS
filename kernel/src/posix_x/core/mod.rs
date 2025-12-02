//! Core POSIX-X Module
//!
//! Central state management and core functionality

pub mod compatibility;
pub mod config;
pub mod fd_table;
pub mod init;
pub mod process_state;

pub use compatibility::{get_compatibility_report, CompatibilityReport, PosixVersion};
pub use config::{get_config, PosixConfig, POSIX_CONFIG};
pub use fd_table::{FdTable, FD_STDERR, FD_STDIN, FD_STDOUT, MAX_FDS};
pub use init::{init, is_initialized, shutdown};
pub use process_state::ProcessState;
