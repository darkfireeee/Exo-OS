// kernel/src/scheduler/policies/mod.rs

pub mod cfs;
pub mod deadline;
pub mod idle;
pub mod realtime;

pub use cfs::{
    normalize_vruntime_on_enqueue, should_preempt_on_wakeup, tick_check_preempt, timeslice_for,
};
pub use deadline::{
    admit_thread, check_deadline_miss, deadline_tick, refresh_deadline, release_thread,
};
pub use idle::{idle_loop, is_idle_thread, mark_idle_thread};
pub use realtime::{fifo_should_preempt, rr_remaining_slice, rr_tick, RR_TIMESLICE_NS};
