// kernel/src/scheduler/core/mod.rs
//
// Module core du scheduler — réexporte toutes les API publiques.

pub mod task;
pub mod preempt;
pub mod runqueue;
pub mod pick_next;
pub mod switch;

pub use task::{
    ThreadControlBlock, ThreadId, ProcessId, CpuId, Priority,
    SchedPolicy, TaskState, DeadlineParams,
    TaskStats, task_flags,
};
pub use preempt::{PreemptGuard, IrqGuard, assert_preempt_disabled, assert_preempt_enabled, MAX_CPUS};
pub use runqueue::{PerCpuRunQueue, RunQueueStats, run_queue, init_percpu, MAX_TASKS_PER_CPU};
pub use pick_next::{pick_next_task, account_time, PickResult};
pub use switch::{context_switch, schedule_yield, check_signal_pending};
