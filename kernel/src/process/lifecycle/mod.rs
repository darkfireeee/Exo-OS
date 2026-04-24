// kernel/src/process/lifecycle/mod.rs
//
// Sous-module lifecycle/ — cycle de vie des processus/threads.

pub mod create;
pub mod exec;
pub mod exit;
pub mod fork;
pub mod reap;
pub mod wait;

pub use create::{create_kthread, create_process, CreateError};
pub use exec::{do_execve, register_elf_loader, ElfLoader, ExecError};
pub use exit::{do_exit, do_exit_thread};
pub use fork::{do_fork, ForkError, ForkFlags};
pub use reap::init_reaper;
pub use wait::{do_waitpid, WaitError, WaitOptions, WaitResult};
