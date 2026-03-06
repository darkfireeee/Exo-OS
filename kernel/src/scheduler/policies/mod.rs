// kernel/src/scheduler/policies/mod.rs

pub mod cfs;
pub mod realtime;
pub mod deadline;
pub mod idle;

pub use cfs::{timeslice_for, normalize_vruntime_on_enqueue, tick_check_preempt, should_preempt_on_wakeup};
pub use realtime::{fifo_should_preempt, rr_tick, rr_remaining_slice, RR_TIMESLICE_NS};
pub use deadline::{admit_thread, release_thread, refresh_deadline, check_deadline_miss, deadline_tick};
pub use idle::{mark_idle_thread, is_idle_thread, idle_loop};
