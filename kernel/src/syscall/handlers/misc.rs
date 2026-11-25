//! # syscall/handlers/misc.rs — Compatibility re-export facade
//!
//! Domain handlers have been segregated into:
//! - `creds_info` : `getpid`, `getppid`, `gettid`, `get*id`, `uname`, `sysinfo`
//! - `prctl`      : `arch_prctl`, `set_tid_address`, `prctl`
//! - `sched`      : `getpriority`, `setpriority`, `sched_*`, `getcpu`

pub use super::creds_info::*;
pub use super::prctl::*;
pub use super::sched::*;
