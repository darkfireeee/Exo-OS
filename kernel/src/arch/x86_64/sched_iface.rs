// kernel/src/arch/x86_64/sched_iface.rs
//
// ═════════════════════════════════════════════════════════════════════════════
// ARCHITECTURE ↔ SCHEDULER BRIDGE — x86_64 (C ABI exports)
// ═════════════════════════════════════════════════════════════════════════════
//
// ## Role
//
// This module exports the `#[no_mangle] extern "C"` functions expected by
// `scheduler/` via `extern "C"` FFI. It constitutes the only coupling point
// between layers:
//
//   arch/ (cross-cutting, can call any layer)
//        ↓ C ABI exports
//   scheduler/ (Layer 1 — CANNOT import arch/ directly, uses FFI)
//
// ## Exported Functions
//
//   arch_send_reschedule_ipi(target_cpu: u32)
//       Sends a reschedule IPI to logical CPU `target_cpu`.
//       Used by `scheduler::smp::migration` to trigger a reschedule
//       on the destination CPU after thread migration.
//
//   arch_set_cpu_pstate(cpu: u32, pstate: u32)
//       Configures the P-state of CPU `cpu` via MSR_IA32_PERF_CTL (0x199).
//       RULE: MSR writes being CPU-local, the effect is only guaranteed if
//       `cpu == arch_current_cpu()`. For a remote CPU, a "pstate change"
//       IPI should be sent — future implementation.
//
// ## Rules (DOC3)
//   SCHED-01: scheduler/ is Layer 1 — cannot import arch/ directly.
//   The FFI bridge here preserves this separation.
//
// ## Synchronization with scheduler/
//   If a signature changes here → update simultaneously:
//     scheduler/smp/migration.rs    (arch_send_reschedule_ipi)
//     scheduler/energy/frequency.rs (arch_set_cpu_pstate)
// ═════════════════════════════════════════════════════════════════════════════

use super::apic;
use super::cpu::features::{cpu_features, CpuVendor};
use super::cpu::msr;
use super::smp::percpu;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// MSR Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Intel MSR IA32_PERF_CTL — bits [7:0] = target P-state
/// Write = request frequency/voltage change to hardware
const MSR_IA32_PERF_CTL: u32 = 0x0000_0199;

/// Intel MSR IA32_PERF_STATUS — bits [15:8] = current P-state
#[allow(dead_code)]
const MSR_IA32_PERF_STATUS: u32 = 0x0000_0198;

/// P-state mask in PERF_CTL (bits [7:0])
const PERF_CTL_PSTATE_MASK: u64 = 0xFF;

// ─────────────────────────────────────────────────────────────────────────────
// Instrumentation
// ─────────────────────────────────────────────────────────────────────────────

static RESCHEDULE_IPIS_SENT: AtomicU64 = AtomicU64::new(0);
static PSTATE_CHANGES: AtomicU64 = AtomicU64::new(0);

pub fn reschedule_ipis_sent() -> u64 {
    RESCHEDULE_IPIS_SENT.load(Ordering::Relaxed)
}
pub fn pstate_changes() -> u64 {
    PSTATE_CHANGES.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// arch_send_reschedule_ipi — C ABI export
// ─────────────────────────────────────────────────────────────────────────────

/// Sends a reschedule IPI to logical CPU `target_cpu`.
///
/// Sequence:
/// 1. Read target CPU's LAPIC ID in `PER_CPU_TABLE[target_cpu]`.
/// 2. Call `apic::ipi::send_ipi_reschedule(lapic_id)`.
///
/// ## Safety Rule
/// Do nothing if `target_cpu >= MAX_CPUS` or if CPU is not online.
/// Do not send an IPI to self (the scheduler already checks this condition,
/// but added here as defense-in-depth).
///
/// ## Caller
/// `scheduler::smp::migration::request_migration()` — triggers reschedule
/// on destination CPU after enqueuing migration.
///
/// # Safety
/// Called by the scheduler upon thread migration request.
#[no_mangle]
pub unsafe extern "C" fn arch_send_reschedule_ipi(target_cpu: u32) {
    // Upper bound: do not access outside table
    if target_cpu as usize >= percpu::MAX_CPUS {
        return;
    }

    // Read per-CPU data of target CPU
    let cpu_data = percpu::per_cpu(target_cpu as usize);

    // Check that CPU is online
    if !cpu_data.online {
        return;
    }

    // Avoid self-IPI (unnecessary and can cause latency on certain APICs)
    let local_id = percpu::current_cpu_id();
    if target_cpu == local_id {
        return;
    }

    let lapic_id = cpu_data.lapic_id as u32;

    // Send reschedule IPI (see arch/x86_64/apic/ipi.rs)
    apic::ipi::send_ipi_reschedule(lapic_id);
    RESCHEDULE_IPIS_SENT.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// arch_set_cpu_pstate — C ABI export
// ─────────────────────────────────────────────────────────────────────────────

/// Configures the P-state of CPU `cpu` via MSR_IA32_PERF_CTL.
///
/// ## P-state encoding (Intel ACPI/HWP)
/// - `pstate = 0`  → Maximum performance (turbo frequency if available)
/// - `pstate = N`  → Frequency decrementation (N * 100 MHz typically)
/// - Value written to PERF_CTL bits [7:0] = frequency ratio × 100 MHz
///
/// ## Local MSR Limitation
/// MSR_IA32_PERF_CTL is a LOCAL CPU register: the written value affects
/// only the CPU executing the WRMSR instruction.
///
/// If `cpu != arch_current_cpu()` → write is ignored (edge case warning).
/// scheduler/energy/frequency.rs must ensure calling this function
/// from the target CPU (via tick handler running on each CPU).
///
/// ## Caller
/// `scheduler::energy::frequency::set_pstate()` — called from tick handler
/// or energy governor running on target CPU.
///
/// # Safety
/// Caller must ensure execution takes place on target processor.
#[no_mangle]
pub unsafe extern "C" fn arch_set_cpu_pstate(cpu: u32, pstate: u32) {
    // Bound check
    if cpu as usize >= percpu::MAX_CPUS {
        return;
    }

    // Verify CPU is online
    let cpu_data = percpu::per_cpu(cpu as usize);
    if !cpu_data.online {
        return;
    }

    // MSR_IA32_PERF_CTL is specific to Intel SpeedStep.
    // Avoids #GP on AMD or third-party hypervisors.
    if cpu_features().vendor != CpuVendor::Intel {
        return;
    }

    // MSR_IA32_PERF_CTL is a local register to the executing CPU.
    // If cpu != current_cpu, ignore to avoid modifying the wrong core's frequency.
    let local_id = percpu::current_cpu_id();
    if cpu != local_id {
        return;
    }

    // Keep only valid P-state bits (bits [7:0])
    let pstate_val = (pstate as u64) & PERF_CTL_PSTATE_MASK;

    // Read current PERF_CTL to modify only P-state bits
    // and preserve other bits (bit 16 = IDA Disengage on some Intels)
    //
    // SAFETY: RDMSR from Ring 0 — MSR_IA32_PERF_CTL supported on all Intel
    // with SpeedStep. On AMD, MSR is different (PERF_CTL = 0xC001_0062)
    // but scheduler calling logic is identical.
    // Mask bits [7:0] and write new P-state.
    let current = msr::read_msr(MSR_IA32_PERF_CTL);
    let new_val = (current & !PERF_CTL_PSTATE_MASK) | pstate_val;

    // SAFETY: WRMSR from Ring 0 — modifies only target frequency
    msr::write_msr(MSR_IA32_PERF_CTL, new_val);
    PSTATE_CHANGES.fetch_add(1, Ordering::Relaxed);
}
