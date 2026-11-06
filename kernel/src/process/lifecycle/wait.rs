// kernel/src/process/lifecycle/wait.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// waitpid() — Waiting for child process termination (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implementation:
//   • Scan registry to find Zombie child.
//   • If not found and WNOHANG absent: block on wait_queue.
//   • SIGCHLD handler wakes up wait_queue.
//   • Return terminated PID + exit code.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pcb::ProcessState;
use crate::process::core::pid::{Pid, PID_ALLOCATOR};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::lifecycle::fork::AddressSpaceCloner;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Options for waitpid (WNOHANG, WUNTRACED, WCONTINUED).
#[derive(Copy, Clone, Default, Debug)]
pub struct WaitOptions(pub u32);

impl WaitOptions {
    /// Immediate return if no child has terminated.
    pub const WNOHANG: u32 = 1 << 0;
    /// Reported when a child is stopped (SIGSTOP).
    pub const WUNTRACED: u32 = 1 << 1;
    /// Reported when a child resumes (SIGCONT).
    pub const WCONTINUED: u32 = 1 << 2;
    /// Wait for any child (pid=-1).
    pub const WALL: u32 = 1 << 3;

    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Result of a successful waitpid.
#[derive(Debug, Clone, Copy)]
pub struct WaitResult {
    /// Terminated (or stopped/continued) child PID.
    pub pid: Pid,
    /// Raw exit status (POSIX wstatus: exit_code << 8).
    pub wstatus: u32,
    /// Termination reason.
    pub reason: WaitReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// Normal termination (exit()).
    Exited,
    /// Killed by a signal.
    Signaled,
    /// Stopped (SIGSTOP).
    Stopped,
    /// Resumed (SIGCONT).
    Continued,
}

/// waitpid error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    /// No matching child (ECHILD).
    NoChild,
    /// No terminated child and WNOHANG set (EAGAIN).
    WouldBlock,
    /// Wait interrupted by signal (EINTR).
    Interrupted,
    /// Invalid requested PID.
    InvalidPid,
}

impl WaitResult {
    /// Encodes a normal exit code (exit_code << 8, signal=0).
    pub fn exited(pid: Pid, code: u8) -> Self {
        Self {
            pid,
            wstatus: (code as u32) << 8,
            reason: WaitReason::Exited,
        }
    }

    /// Encodes a termination code by signal (POSIX code).
    pub fn signaled(pid: Pid, sig: u8, core_dumped: bool) -> Self {
        let dump_bit = if core_dumped { 0x80u32 } else { 0 };
        Self {
            pid,
            wstatus: (sig as u32) | dump_bit,
            reason: WaitReason::Signaled,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WaitTable — waiting parents table
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::sync::spinlock::SpinLock;
use crate::scheduler::sync::wait_queue::WaitQueue;

/// Entry in waiting parents table.
#[allow(dead_code)]
struct WaitEntry {
    /// Waiting parent PID.
    parent_pid: u32,
    /// Awaited child PID (0 = any child of parent).
    child_pid: u32,
    /// Result posted by SIGCHLD / exit().
    result_pid: core::sync::atomic::AtomicU32,
    result_code: core::sync::atomic::AtomicU32,
}

/// Active wait table — max 512 simultaneous entries.
#[allow(dead_code)]
struct WaitTable {
    entries: SpinLock<[Option<WaitEntry>; 512]>,
    queue: WaitQueue,
}

// SAFETY: WaitTable accessible across multiple CPUs, protected by SpinLock.
unsafe impl Sync for WaitTable {}

static WAIT_TABLE: WaitQueue = WaitQueue::new();

// ─────────────────────────────────────────────────────────────────────────────
// do_waitpid
// ─────────────────────────────────────────────────────────────────────────────

/// Waits for a child process to terminate.
///
/// # Arguments
/// * `caller_pid` — calling process PID.
/// * `wait_pid` — PID to wait for (−0 = any child).
/// * `opts` — wait flags.
/// * `caller_tcb` — calling thread TCB (for blocking).
pub fn do_waitpid(
    caller_pid: Pid,
    wait_pid: i32,
    opts: WaitOptions,
    caller_tcb: &crate::scheduler::core::task::ThreadControlBlock,
) -> Result<WaitResult, WaitError> {
    // Fast scan: look for an already Zombie child in the registry.
    let result = reap_zombie_child(caller_pid, wait_pid, opts);
    if let Some(r) = result {
        return Ok(r);
    }
    // No Zombie child found.
    if opts.has(WaitOptions::WNOHANG) {
        // Check if at least one child exists.
        if has_children(caller_pid) {
            return Err(WaitError::WouldBlock);
        } else {
            return Err(WaitError::NoChild);
        }
    }
    // Block — wait for notification via SIGCHLD/wait_queue.
    // NOTE: wait_queue uses EmergencyPool (RULE WAITQ-01).
    // Loop: spurious wakeups are tolerated.
    loop {
        // Attempt to reap immediately before interrupting on pending signal
        if let Some(r) = reap_zombie_child(caller_pid, wait_pid, opts) {
            return Ok(r);
        }
        // Check pending signal (EINTR if non-SIGCHLD signal).
        if caller_tcb.has_signal_pending() {
            return Err(WaitError::Interrupted);
        }
        // Put self on wait in WAIT_TABLE.
        // SAFETY: WaitQueue EmergencyPool (WAITQ-01); caller_tcb current TCB, no active &mut alias.
        unsafe {
            WAIT_TABLE.wait_interruptible(caller_tcb as *const _ as *mut _);
        }
        // Retry.
        if let Some(r) = reap_zombie_child(caller_pid, wait_pid, opts) {
            return Ok(r);
        }
        if !has_children(caller_pid) {
            return Err(WaitError::NoChild);
        }
    }
}

/// Wakes up all waiting parents (called by SIGCHLD delivery).
pub fn wake_waiting_parents(child_pid: Pid, parent_pid: Pid) {
    let _ = (child_pid, parent_pid);
    WAIT_TABLE.notify_all();
}

#[derive(Clone, Copy)]
struct ZombieCandidate {
    pid: Pid,
    code: u32,
}

fn reap_zombie_child(parent_pid: Pid, wait_pid: i32, opts: WaitOptions) -> Option<WaitResult> {
    let candidate = scan_zombie_children(parent_pid, wait_pid, opts)?;
    let pcb_box = PROCESS_REGISTRY.remove(candidate.pid).ok()?;

    if pcb_box.ppid() != parent_pid || pcb_box.state() != ProcessState::Zombie {
        let _ = PROCESS_REGISTRY.insert(pcb_box);
        return None;
    }

    let addr_space_ptr = pcb_box.address_space.load(Ordering::Acquire);
    let pcb_flags = pcb_box.flags.load(Ordering::Acquire);
    let is_vfork_shared = pcb_flags & crate::process::core::pcb::process_flags::VFORK_SHARED_AS != 0;
    {
        let mut files = pcb_box.files.lock();
        files.close_all_noalloc();
    }
    pcb_box.set_state(ProcessState::Dead);
    drop(pcb_box);

    if addr_space_ptr != 0 && !is_vfork_shared {
        crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER
            .free_addr_space(addr_space_ptr);
    }
    // FIX-SEC-T1.1: reset zero-trust restriction state to zero BEFORE
    // recycling PID — a future process reusing this number must start without
    // restrictions inherited from the deceased.
    crate::security::zero_trust::clear_process_restrictions(candidate.pid.0);
    PID_ALLOCATOR.free(candidate.pid.0);

    let raw_code = candidate.code;
    let wait_res = if raw_code & 0x8000_0000 != 0 {
        let sig = (raw_code & 0x7F) as u8;
        let core = (raw_code & 0x80) != 0;
        WaitResult::signaled(candidate.pid, sig, core)
    } else {
        WaitResult::exited(candidate.pid, (raw_code & 0xFF) as u8)
    };
    Some(wait_res)
}

/// Scans registry to find a Zombie child of parent.
fn scan_zombie_children(
    parent_pid: Pid,
    wait_pid: i32,
    _opts: WaitOptions,
) -> Option<ZombieCandidate> {
    let any_child = wait_pid <= 0;
    let mut found: Option<ZombieCandidate> = None;

    PROCESS_REGISTRY.for_each(|pcb| {
        if found.is_some() {
            return;
        }
        let ppid = Pid(pcb.ppid.load(Ordering::Relaxed));
        if ppid != parent_pid {
            return;
        }
        if !any_child && pcb.pid.0 != wait_pid as u32 {
            return;
        }
        if pcb.state() == ProcessState::Zombie {
            let code = pcb.exit_code.load(Ordering::Acquire);
            found = Some(ZombieCandidate { pid: pcb.pid, code });
        }
    });
    found
}

/// Checks if parent has at least one active child.
fn has_children(parent_pid: Pid) -> bool {
    let mut found = false;
    PROCESS_REGISTRY.for_each(|pcb| {
        if found {
            return;
        }
        let ppid = Pid(pcb.ppid.load(Ordering::Relaxed));
        if ppid == parent_pid {
            found = true;
        }
    });
    found
}
