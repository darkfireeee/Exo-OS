//! # process/core/pcb/mod.rs — ProcessControlBlock (Exo-OS Layer 1.5)
//!
//! Module wiring + `ProcessControlBlock` struct definition.
//! Method implementations reside in `pcb_impl.rs`.

pub mod file_table;
mod pcb_impl;
pub mod types;

pub use file_table::{FileDescriptor, OpenFileTable};
pub use types::{
    process_flags, Credentials, ProcessFlags, ProcessState, DEFAULT_UMASK, PROCESS_NAME_LEN,
};

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize};

use crate::process::core::pid::Pid;
use crate::process::signal::default::SigHandlerTable;
use crate::scheduler::core::task::ThreadId;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::table::CapTable;

/// Process Control Block — central per-process structure.
///
/// Shared between all threads of the process.
/// Protected by its own spinlock for non-atomic mutable fields.
pub struct ProcessControlBlock {
    // ── Identity ──────────────────────────────────────────────────────────────
    pub pid: Pid,
    pub name: SpinLock<[u8; PROCESS_NAME_LEN]>,
    pub ppid: AtomicU32,
    pub tgid: Pid,
    pub sid: AtomicU32,
    pub pgid: AtomicU32,

    // ── State ─────────────────────────────────────────────────────────────────
    pub state: AtomicU32, // ProcessState as u32
    pub flags: AtomicU32,
    pub exit_code: AtomicU32,
    pub umask: AtomicU32,

    // ── Threads ───────────────────────────────────────────────────────────────
    pub thread_count: AtomicU32,
    pub max_threads: AtomicU32,
    pub main_thread: ThreadId,
    pub threads: SpinLock<Vec<*mut crate::process::core::tcb::ProcessThread>>,

    // ── Resources ─────────────────────────────────────────────────────────────
    pub creds: SpinLock<Credentials>,
    pub euid_atomic: AtomicU32,
    pub files: Arc<SpinLock<OpenFileTable>>,

    // ── Virtual memory ────────────────────────────────────────────────────────
    pub address_space: AtomicUsize, // *mut opaque
    pub as_lock: SpinLock<()>,
    pub cr3: AtomicU64,
    pub brk_current: AtomicU64,
    pub brk_start: AtomicU64,

    // ── Performance counters ──────────────────────────────────────────────────
    pub utime_ns: AtomicU64,
    pub stime_ns: AtomicU64,
    pub major_faults: AtomicU64,
    pub minor_faults: AtomicU64,
    pub io_read_bytes: AtomicU64,
    pub io_write_bytes: AtomicU64,

    // ── Signals ───────────────────────────────────────────────────────────────
    pub sig_handlers: SpinLock<SigHandlerTable>,
    pub signal_tcb_vaddr: AtomicU64,
    pub main_thread_rawptr: AtomicPtr<crate::process::core::tcb::ProcessThread>,

    // ── Namespaces ────────────────────────────────────────────────────────────
    pub pid_ns: u32,
    pub mnt_ns: u32,
    pub net_ns: u32,
    pub uts_ns: u32,
    pub user_ns: u32,

    // ── Capabilities (FIX-P2-A03) ─────────────────────────────────────────
    pub cap_table: Box<CapTable>,
}

// SAFETY: ProcessControlBlock is shared among threads of the same process.
// All mutable fields are either atomic or protected by SpinLock.
unsafe impl Send for ProcessControlBlock {}
unsafe impl Sync for ProcessControlBlock {}
