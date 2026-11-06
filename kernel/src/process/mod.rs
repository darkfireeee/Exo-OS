// kernel/src/process/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PROCESS MODULE — Layer 1.5: Process and thread management
// ═══════════════════════════════════════════════════════════════════════════════
//
// ARCHITECTURAL POSITION (docs/refonte/DOC4):
//   Layer 1.5 — depends on memory/ (Layer 0) + scheduler/ (Layer 1)
//   Called by: ipc/, fs/ (via traits), arch/syscall
//   FORBIDDEN: use crate::fs, use crate::ipc (except via abstract traits)
//   signal/: HERE only (RULE SIGNAL-01 DOC1)
//
// SUBMODULES:
//   core/       — pid.rs, pcb.rs, tcb.rs (process extensions), registry.rs
//   lifecycle/  — create, fork, exec, exit, wait, reap
//   thread/     — creation, join, detach, TLS, pthread_compat
//   signal/     — delivery, handler, mask, queue, default  ← moved from scheduler/
//   state/      — state machine, wakeup (DmaWakeupHandler impl)
//   group/      — session, pgrp, job_control
//   namespace/  — pid_ns, mount_ns, net_ns, uts_ns, user_ns
//   resource/   — rlimit, usage, cgroup
//
// INIT SEQUENCE (global step 20):
//   1. core::pid::init()
//   2. core::registry::init()
//   3. lifecycle::reap::init_reaper()
//   4. register_with_oom()
//   5. state::wakeup::register_with_dma()
//   6. resource::cgroup::init() (idempotent; root already prepared before runqueues)
//
// ABSOLUTE RULES:
//   • PROC-01: exec() via abstract ElfLoader trait
//   • PROC-02: DmaWakeupHandler impl in state/wakeup.rs
//   • PROC-03: signal/ managed entirely here
//   • PROC-04: signal_pending WRITTEN by process/signal/, READ by scheduler
//   • PROC-05: ProcessThread TCB ≤ 256 additional bytes
//   • PROC-06: Signal delivery: on return to userspace ONLY
//   • PROC-07: zombie reaper = dedicated kthread
//   • PROC-08: fork() flushes parent TLB BEFORE return
//   • PROC-09: namespace/ = full isolation PID/net/mount
//   • PROC-10: dma_completion_result in scheduler TCB (AtomicU8)
//   • unsafe → // SAFETY: mandatory (regle_bonus.md)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod auxv;
pub mod core;
pub mod group;
pub mod lifecycle;
pub mod namespace;
pub mod resource;
pub mod signal;
pub mod state;
pub mod thread;

use self::signal::default::Signal;
use self::signal::delivery::send_signal_to_pid;
use crate::memory::OomKillCandidate;
use crate::rust_core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Main re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use self::core::pcb::{ProcessControlBlock, ProcessFlags, ProcessState};
pub use self::core::pid::{Pid, PidAllocator, Tid, PID_ALLOCATOR, TID_ALLOCATOR};
pub use self::core::registry::{ProcessRegistry, PROCESS_REGISTRY};
pub use self::core::tcb::{ProcessThread, ThreadAddress};
pub use self::lifecycle::exec::{load_elf_for_boot, register_elf_loader, ElfLoader, ExecError};
pub use self::signal::delivery::handle_pending_signals;
pub use self::state::wakeup::{register_with_dma, PROCESS_WAKEUP_HANDLER};

// ─────────────────────────────────────────────────────────────────────────────
// Initialization parameters
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for `process::init()`.
pub struct ProcessInitParams {
    /// Maximum number of concurrent processes.
    pub max_pids: usize,
    /// Maximum number of concurrent threads.
    pub max_tids: usize,
    /// Kernel stack per thread (bytes, multiple of PAGE_SIZE).
    pub kstack_size: usize,
}

impl Default for ProcessInitParams {
    fn default() -> Self {
        Self {
            max_pids: 32768,
            max_tids: 131072,
            kstack_size: self::core::tcb::KSTACK_SIZE,
        }
    }
}

/// Initializes the process subsystem.
///
/// Called from `kernel_main` AFTER `scheduler::init()`.
///
/// # Safety
/// Called once from the BSP before activating APs.
pub unsafe fn init(params: &ProcessInitParams) {
    self::core::pid::init(params.max_pids, params.max_tids);
    self::core::registry::init(params.max_pids);
    self::lifecycle::reap::init_reaper();
    register_with_oom();
    self::state::wakeup::register_with_dma();
    self::resource::cgroup::init();
}

fn oom_send_sigkill(pid: u64) -> bool {
    if pid == 0 || pid > u32::MAX as u64 {
        return false;
    }
    send_signal_to_pid(Pid(pid as u32), Signal::SIGKILL).is_ok()
}

fn oom_candidates(buf: &mut [OomKillCandidate]) -> usize {
    let mut n = 0usize;
    PROCESS_REGISTRY.for_each(|pcb| {
        if n >= buf.len() || pcb.pid.0 <= 1 {
            return;
        }
        let state = pcb.state();
        if matches!(state, ProcessState::Zombie | ProcessState::Dead) || pcb.is_exiting() {
            return;
        }

        let brk_start = pcb.brk_start.load(Ordering::Acquire);
        let brk_current = pcb.brk_current.load(Ordering::Acquire);
        let brk_pages = brk_current.saturating_sub(brk_start) / crate::memory::PAGE_SIZE as u64;
        let fault_score = pcb
            .major_faults
            .load(Ordering::Relaxed)
            .saturating_mul(16)
            .saturating_add(pcb.minor_faults.load(Ordering::Relaxed));
        let thread_score = (pcb.thread_count.load(Ordering::Acquire) as u64).saturating_mul(128);
        let io_score = pcb
            .io_read_bytes
            .load(Ordering::Relaxed)
            .saturating_add(pcb.io_write_bytes.load(Ordering::Relaxed))
            / 4096;

        buf[n] = OomKillCandidate {
            pid: pcb.pid.0 as u64,
            oom_score: brk_pages
                .saturating_mul(4)
                .saturating_add(fault_score)
                .saturating_add(thread_score)
                .saturating_add(io_score)
                .max(1),
            vm_rss: brk_pages,
            name: [0; 16],
        };
        n += 1;
    });
    n
}

fn register_with_oom() {
    crate::memory::register_oom_kill_sender(oom_send_sigkill);
    crate::memory::register_oom_candidate_provider(oom_candidates);
}

// ─────────────────────────────────────────────────────────────────────────────
// Process utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Check if a process with given PID is alive (registered and not reaped).
///
/// Used by IRQ handler purge logic (CORR-51) and watchdog.
/// Returns true if PID exists in PROCESS_REGISTRY.
#[inline]
pub fn is_alive(pid: u32) -> bool {
    PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .is_some()
}
