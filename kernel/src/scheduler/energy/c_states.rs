// kernel/src/scheduler/energy/c_states.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// C-States — CPU sleep state management
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE CSTATE-01: fetch_min for RT constraint.
//   When an RT thread is active, the allowed C-state cannot exceed C1.
//   The update uses `fetch_min` to ensure that the most restrictive C-state
//   is always respected.
//
// Supported C-states:
//   C0 = active (no action)
//   C1 = HLT (a few microseconds wakeup latency)
//   C2 = MWAIT + hint (tens of µs)
//   C3 = deep sleep (hundreds of µs — forbidden in the presence of RT threads)
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::smp::topology::MAX_CPUS;
use core::sync::atomic::{AtomicU8, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// C-state levels
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CState {
    C0 = 0, // Active
    C1 = 1, // HLT
    C2 = 2, // MWAIT/light
    C3 = 3, // Deep sleep
}

impl CState {
    /// Estimated exit latency (ns).
    pub fn exit_latency_ns(self) -> u64 {
        match self {
            CState::C0 => 0,
            CState::C1 => 1_000,   // 1µs
            CState::C2 => 10_000,  // 10µs
            CState::C3 => 200_000, // 200µs
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU C-state constraint (fetch_min — RULE CSTATE-01)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum C-state constraint per CPU (0 = no constraint = C3 allowed).
/// An RT thread enforces C1 using `fetch_min(C1 as u8)`.
static CSTATE_MAX: [AtomicU8; MAX_CPUS] =
    [const { AtomicU8::new(CState::C3 as u8) }; MAX_CPUS];

/// BUG-FIX G: count of active RT threads per CPU.
/// Before this fix, `release_rt_constraint()` restored C3 immediately
/// even if other RT threads were still running on the same CPU.
static RT_ACTIVE_COUNT: [AtomicU8; MAX_CPUS] =
    [const { AtomicU8::new(0) }; MAX_CPUS];

/// Applies RT constraint: forces maximum C1 on CPU `cpu`.
/// Uses `fetch_min` to respect RULE CSTATE-01.
pub fn constrain_rt(cpu: usize) {
    if cpu < MAX_CPUS {
        RT_ACTIVE_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
        CSTATE_MAX[cpu].fetch_min(CState::C1 as u8, Ordering::AcqRel);
    }
}

/// Releases RT constraint on CPU `cpu`.
/// Only restores C3 when the last RT thread leaves the CPU.
pub fn release_rt_constraint(cpu: usize) {
    if cpu < MAX_CPUS {
        // Decrement RT counter, saturating at 0.
        let prev = RT_ACTIVE_COUNT[cpu]
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                Some(v.saturating_sub(1))
            })
            .unwrap_or(0);
        // Restore C3 only if this was the last active RT thread.
        if prev <= 1 {
            CSTATE_MAX[cpu].store(CState::C3 as u8, Ordering::Release);
        }
    }
}

/// Returns the maximum allowed C-state on CPU `cpu`.
pub fn max_allowed_cstate(cpu: usize) -> CState {
    if cpu >= MAX_CPUS {
        return CState::C1;
    }
    match CSTATE_MAX[cpu].load(Ordering::Acquire) {
        0 => CState::C0,
        1 => CState::C1,
        2 => CState::C2,
        _ => CState::C3,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Optimal C-state selection
// ─────────────────────────────────────────────────────────────────────────────

/// Selects the optimal C-state for CPU `cpu` according to expected idle
/// time `idle_ns`.
///
/// Rule: enter a C-state only if energy savings outweigh exit latency
/// (break-even point = exit_latency × overhead_factor).
pub fn select_cstate(cpu: usize, idle_ns: u64) -> CState {
    let max = max_allowed_cstate(cpu);
    let candidates = [CState::C3, CState::C2, CState::C1, CState::C0];
    for cs in candidates {
        if cs > max {
            continue;
        }
        // Enter this C-state only if saving at least 2× the exit latency.
        let threshold = cs.exit_latency_ns().saturating_mul(2);
        if idle_ns >= threshold {
            return cs;
        }
    }
    CState::C0
}

// ─────────────────────────────────────────────────────────────────────────────
// C-state entry
// ─────────────────────────────────────────────────────────────────────────────

/// Enters requested C-state on current CPU.
///
/// # Safety
/// IRQs must be enabled before HLT/MWAIT instruction.
pub unsafe fn enter_cstate(cs: CState) {
    match cs {
        CState::C0 => {}
        CState::C1 => {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
        CState::C2 | CState::C3 => {
            // MWAIT with appropriate hint.
            let hint: u32 = match cs {
                CState::C2 => 0x10, // C2 sub-state 0
                CState::C3 => 0x20, // C3 sub-state 0
                _ => 0x00,
            };
            // MONITOR/MWAIT: monitors a memory range.
            // In practice, monitors get_current_tcb().need_resched.
            core::arch::asm!(
                "monitor",       // monitors [rax]
                "mwait",         // waits for event or MONITOR range
                in("eax") 0u32,  // MONITOR address (simplified)
                in("ecx") 0u32,  // extension hints = 0
                in("edx") hint,  // hints
                options(nomem, nostack, preserves_flags),
            );
        }
    }
}

/// Initializes C-states subsystem for `nr_cpus` CPUs.
///
/// # Safety
/// Called once from scheduler::init().
pub unsafe fn init(nr_cpus: usize) {
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        CSTATE_MAX[cpu].store(CState::C3 as u8, Ordering::Relaxed);
        RT_ACTIVE_COUNT[cpu].store(0, Ordering::Relaxed);
    }
}
