// kernel/src/scheduler/core/mod.rs
//
// Module core du scheduler — réexporte toutes les API publiques.

pub mod boot_idle;
pub mod pick_next;
pub mod preempt;
pub mod runqueue;
pub mod switch;
pub mod task;

pub use boot_idle::{bind_boot_idle_threads, ensure_boot_idle_tcb, publish_current_boot_idle};
pub use pick_next::{account_time, pick_next_task, PickResult};
pub use preempt::{
    assert_preempt_disabled, assert_preempt_enabled, IrqGuard, PreemptGuard, MAX_CPUS,
};
pub use runqueue::{init_percpu, run_queue, PerCpuRunQueue, RunQueueStats, MAX_TASKS_PER_CPU};
pub use switch::{check_signal_pending, context_switch, schedule_yield};
pub use task::{
    task_flags, CpuId, DeadlineParams, Priority, ProcessId, SchedPolicy, TaskState, TaskStats,
    ThreadControlBlock, ThreadId,
};
