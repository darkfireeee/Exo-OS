// kernel/src/process/lifecycle/mod.rs
//
// Sous-module lifecycle/ — cycle de vie des processus/threads.

pub mod create;
pub mod fork;
pub mod exec;
pub mod exit;
pub mod wait;
pub mod reap;

pub use create::{create_process, create_kthread, CreateError};
pub use fork::{do_fork, ForkError, ForkFlags};
pub use exec::{do_execve, register_elf_loader, ElfLoader, ExecError};
pub use exit::{do_exit, do_exit_thread};
pub use wait::{do_waitpid, WaitOptions, WaitResult, WaitError};
pub use reap::init_reaper;
