//! # process/core/pcb/types.rs — Types and flags for ProcessControlBlock
//!
//! Defines the ProcessState state machine, process flags,
//! POSIX identifiers (Credentials), and fundamental constants.

use alloc::alloc::{alloc, Layout};
use alloc::boxed::Box;

pub fn try_box_new<T>(value: T) -> Option<Box<T>> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return None;
    }

    // SAFETY: `layout` is built for T; null is handled and the initialized
    // allocation is immediately owned by Box.
    let raw = unsafe { alloc(layout) as *mut T };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` points to a valid allocation for T and is not aliased.
    unsafe {
        raw.write(value);
        Some(Box::from_raw(raw))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessState — process state machine
// ─────────────────────────────────────────────────────────────────────────────

/// Process state (distinct from individual thread state).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ProcessState {
    /// Being created (fork / execve not completed).
    Creating = 0,
    /// At least one active thread.
    Running = 1,
    /// All threads blocked.
    Sleeping = 2,
    /// Stopped via SIGSTOP (all threads Stopped).
    Stopped = 3,
    /// exit() called, waiting for waitpid() from parent.
    Zombie = 4,
    /// Resources freed (after reap).
    Dead = 5,
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessFlags
// ─────────────────────────────────────────────────────────────────────────────

pub mod process_flags {
    /// fork() via copy-on-write (CLONE_VM absent).
    pub const FORKED: u32 = 1 << 0;
    /// execve() performed at least once.
    pub const EXEC_DONE: u32 = 1 << 1;
    /// Session leader process.
    pub const SESSION_LEADER: u32 = 1 << 2;
    /// Daemonized process (no ctty).
    pub const DAEMON: u32 = 1 << 3;
    /// Undergoing exit — do not send new signals.
    pub const EXITING: u32 = 1 << 4;
    /// Setuid active.
    pub const SETUID: u32 = 1 << 5;
    /// Setgid active.
    pub const SETGID: u32 = 1 << 6;
    /// Core dump prohibited (prctl(PR_SET_DUMPABLE, 0)).
    pub const NO_DUMP: u32 = 1 << 7;
    /// Process subjected to ptrace.
    pub const TRACED: u32 = 1 << 8;
    /// Process in a PID namespace (non-init).
    pub const IN_PID_NS: u32 = 1 << 9;
    /// Vfork pending — release blocked parent.
    pub const VFORK_DONE: u32 = 1 << 10;
    /// vfork() child temporarily sharing parent address space until exec/exit.
    pub const VFORK_SHARED_AS: u32 = 1 << 11;
}

pub use process_flags as ProcessFlags;

pub const PROCESS_NAME_LEN: usize = 16;
pub const MAX_THREADS_PER_PROCESS: usize = 64;
pub const DEFAULT_UMASK: u32 = 0o022;

// ─────────────────────────────────────────────────────────────────────────────
// Credentials — UID/GID
// ─────────────────────────────────────────────────────────────────────────────

/// Process credentials (cloned across fork, replaced by setuid/setgid).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct Credentials {
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    /// Filesystem UID/GID (Linux-compat).
    pub fsuid: u32,
    pub fsgid: u32,
}

impl Credentials {
    pub const ROOT: Self = Self {
        uid: 0,
        gid: 0,
        euid: 0,
        egid: 0,
        suid: 0,
        sgid: 0,
        fsuid: 0,
        fsgid: 0,
    };

    pub fn new(uid: u32, gid: u32) -> Self {
        Self {
            uid,
            gid,
            euid: uid,
            egid: gid,
            suid: uid,
            sgid: gid,
            fsuid: uid,
            fsgid: gid,
        }
    }

    #[inline(always)]
    pub fn is_root(&self) -> bool {
        self.euid == 0
    }
}
