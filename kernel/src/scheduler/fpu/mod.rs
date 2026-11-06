// kernel/src/scheduler/fpu/mod.rs
//
// FPU scheduler module — logical separation of state / ASM instructions.
// arch/x86_64/cpu/fpu.rs = raw instructions (XSAVE/XRSTOR/FXSAVE).
// This module = policy (lazy, when to save, for which thread).

pub mod lazy;
pub mod save_restore;
pub mod state;

pub use lazy::{cr0_clear_ts, cr0_set_ts, handle_nm_exception, init, mark_fpu_not_loaded};
pub use save_restore::{alloc_fpu_state, free_fpu_state, xrstor_for, xsave_current};
pub use state::{detect_xsave_size, FpuState, FXSAVE_SIZE, XSAVE_AREA_SIZE};
