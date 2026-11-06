// kernel/src/scheduler/energy/frequency.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CPU frequency management — P-states and DVFS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RT scheduling adjusts time budgets taking into account the current
// frequency: a thread whose execution must complete in 1ms at 3GHz
// needs 1.5ms at 2GHz.
//
// There are up to MAX_PSTATES frequency levels available, numbered P0
// (maximum link) to Pn (minimum).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::smp::topology::MAX_CPUS;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const MAX_PSTATES: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// P-state table (nominal frequencies in MHz)
// ─────────────────────────────────────────────────────────────────────────────

static mut PSTATE_TABLE: [u32; MAX_PSTATES] = [0u32; MAX_PSTATES];
static PSTATE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Registers table of available P-states (frequencies in MHz, highest
/// to lowest). Called from platform during boot.
///
/// # Safety
/// Called once before any frequency usage.
pub unsafe fn set_pstate_table(freqs_mhz: &[u32]) {
    let n = freqs_mhz.len().min(MAX_PSTATES);
    PSTATE_TABLE[..n].copy_from_slice(&freqs_mhz[..n]);
    PSTATE_COUNT.store(n as u32, Ordering::Release);
}

/// Frequency of P-state `p` in MHz. Returns 0 if out of bounds.
pub fn pstate_freq_mhz(p: usize) -> u32 {
    let n = PSTATE_COUNT.load(Ordering::Relaxed) as usize;
    if p < n {
        unsafe { PSTATE_TABLE[p] }
    } else {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Current P-state per CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Current P-state of each CPU (index in PSTATE_TABLE; 0 = highest frequency).
static CURRENT_PSTATE: [AtomicU32; MAX_CPUS] =
    [const { AtomicU32::new(0) }; MAX_CPUS];

pub fn current_pstate(cpu: usize) -> u32 {
    if cpu < MAX_CPUS {
        CURRENT_PSTATE[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Returns current frequency of CPU `cpu` in MHz.
pub fn current_freq_mhz(cpu: usize) -> u32 {
    pstate_freq_mhz(current_pstate(cpu) as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI to arch to change P-state
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    fn arch_set_cpu_pstate(cpu: u32, pstate: u32);
}

/// Requests transitioning CPU `cpu` to P-state `p`.
///
/// # Safety
/// Called with preemption disabled.
pub unsafe fn set_pstate(cpu: usize, p: u32) {
    if cpu >= MAX_CPUS {
        return;
    }
    let n = PSTATE_COUNT.load(Ordering::Relaxed);
    let p = p.min(n.saturating_sub(1));
    CURRENT_PSTATE[cpu].store(p, Ordering::Relaxed);
    arch_set_cpu_pstate(cpu as u32, p);
}

// ─────────────────────────────────────────────────────────────────────────────
// RT budget adjustment according to frequency
// ─────────────────────────────────────────────────────────────────────────────

/// Adjusts a budget in ns defined at maximum frequency (P0) to the current
/// frequency of CPU `cpu`.
///
/// budget_at_current_freq = budget_at_max × (freq_max / freq_current)
/// (with freq_max = PSTATE_TABLE[0])
pub fn scale_budget_ns(budget_ns: u64, cpu: usize) -> u64 {
    let freq_max = pstate_freq_mhz(0);
    let freq_cur = current_freq_mhz(cpu);
    if freq_cur == 0 || freq_max == 0 {
        return budget_ns;
    }
    // Integer-safe scaling: budget × freq_max / freq_cur
    ((budget_ns as u128 * freq_max as u128) / freq_cur as u128) as u64
}

pub static PSTATE_CHANGES: AtomicU64 = AtomicU64::new(0);
