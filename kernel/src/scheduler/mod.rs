// kernel/src/scheduler/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Scheduler Module — entry point and initialization sequence
// ═══════════════════════════════════════════════════════════════════════════════
//
// Layer 1: depends ONLY on memory/ (Layer 0).
// No imports from process/, ipc/, fs/, security/.
//
// Init sequence (DOC3):
//   1. preempt::init()
//   2. runqueue::init_percpu()
//   3. fpu::save_restore::init()   (detects XSAVE size)
//   4. fpu::lazy::init()           (CR0.TS=1)
//   5. timer::clock::init()        (TSC calibration)
//   6. timer::tick::init()         (HZ=1000)
//   7. timer::hrtimer::init()
//   8. timer::deadline_timer::init()
//   9. sync::wait_queue::init()    (verifies EmergencyPool)
//  10. energy::c_states::init()
//  11. smp::topology_init()
// ═══════════════════════════════════════════════════════════════════════════════

pub mod core;
pub mod energy;
pub mod fpu;
pub mod policies;
pub mod smp;
pub mod stats;
pub mod sync;
pub mod timer;

// Main re-exports.
pub use self::core::pick_next::pick_next_task;
pub use self::core::preempt::{IrqGuard, PreemptGuard};
pub use self::core::runqueue::{init_percpu as rq_init_percpu, run_queue};
pub use self::core::switch::{
    block_current_thread, context_switch, current_thread_raw, schedule_block, schedule_yield,
    wake_enqueue,
};
pub use self::core::task::{
    CpuId, ProcessId, SchedPolicy, TaskState, ThreadControlBlock, ThreadId,
};
pub use self::timer::clock::monotonic_ns;
pub use self::timer::tick::scheduler_tick;

// ─────────────────────────────────────────────────────────────────────────────
// Global initialization sequence
// ─────────────────────────────────────────────────────────────────────────────

/// Scheduler initialization parameters.
pub struct SchedInitParams {
    /// Number of logical CPUs.
    pub nr_cpus: usize,
    /// Number of NUMA nodes.
    pub nr_nodes: usize,
    // RULE SCHED-INIT-01: tsc_hz removed — comes from ktime_get_ns() via time_init().
    // scheduler::init() is called AFTER time::time_init().
}

impl Default for SchedInitParams {
    fn default() -> Self {
        Self {
            nr_cpus: 1,
            nr_nodes: 1,
        }
    }
}

/// Initializes the scheduler subsystem.
///
/// Must be called by the BSP AFTER `memory::init()` has executed
/// (the EmergencyPool must be available prior to `sync::wait_queue::init()`).
///
/// # Safety
/// Called once, from the BSP, before APs are activated.
pub unsafe fn init(params: &SchedInitParams) {
    // BUG-FIX H: clamp nr_cpus to MAX_CPUS to prevent out-of-bounds access
    // on PER_CPU_RQ (64 entries) if the caller passes nr_cpus > 64.
    let nr_cpus = params
        .nr_cpus
        .clamp(1, crate::scheduler::core::preempt::MAX_CPUS);
    let nr_nodes = params.nr_nodes.max(1);
    // tsc_hz comes from ktime_get_ns() (time_init() called before) — RULE SCHED-INIT-01.

    // Step 1 — Preemption counters.
    self::core::preempt::init();

    // Step 2 — Per-CPU run queues.
    self::core::runqueue::init_percpu(nr_cpus);

    // Step 3 — XSAVE detection.
    self::fpu::save_restore::init();

    // Step 4 — Lazy FPU (CR0.TS=1 on the BSP).
    self::fpu::lazy::init();

    // Step 5 — Scheduler clock (delegates to ktime_get_ns — ARCH-TIME-01).
    self::timer::clock::init(0);
    self::timer::tick::init(nr_cpus);

    // Step 7 — HRTimers.
    self::timer::hrtimer::init(nr_cpus);

    // Step 8 — Deadline timers.
    self::timer::deadline_timer::init(nr_cpus);

    // Step 8b — scheduler tick takes ownership of the LAPIC timer.
    crate::arch::x86_64::apic::local_apic::init_scheduler_timer(
        crate::arch::x86_64::idt::VEC_IRQ_TIMER,
    );

    // Step 9 — Wait queues (verifies EmergencyPool is ready).
    self::sync::wait_queue::init();

    // Step 10 — C-states.
    self::energy::c_states::init(nr_cpus);

    // Step 11 — SMP topology.
    self::smp::topology_init(nr_cpus, nr_nodes);
}

/// Initializes the scheduler on an AP (Application Processor).
///
/// Called by each AP after startup.
///
/// # Safety
/// Called from the boot context of each AP.
pub unsafe fn init_ap(cpu_id: u32) {
    // CR0.TS=1 on this AP (lazy FPU).
    self::fpu::lazy::init();
    self::timer::tick::reset_tick_counters(cpu_id as usize);

    // This CPU's run queue is already initialized by init_percpu().
}
