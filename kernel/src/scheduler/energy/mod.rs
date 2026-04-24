// kernel/src/scheduler/energy/mod.rs

pub mod c_states;
pub mod frequency;
pub mod power_profile;

pub use c_states::{constrain_rt, enter_cstate, release_rt_constraint, select_cstate, CState};
pub use frequency::{current_freq_mhz, scale_budget_ns, set_pstate};
pub use power_profile::energy_score;
