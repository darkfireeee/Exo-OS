// kernel/src/scheduler/fpu/mod.rs
//
// Module FPU du scheduler — séparation logique état / instructions ASM.
// arch/x86_64/cpu/fpu.rs = instructions brutes (XSAVE/XRSTOR/FXSAVE).
// Ce module = politique (lazy, quand sauvegarder, pour quel thread).

pub mod lazy;
pub mod save_restore;
pub mod state;

pub use lazy::{cr0_clear_ts, cr0_set_ts, handle_nm_exception, init, mark_fpu_not_loaded};
pub use save_restore::{alloc_fpu_state, xrstor_for, xsave_current};
pub use state::{detect_xsave_size, FpuState, FXSAVE_SIZE, XSAVE_AREA_SIZE};
