// kernel/src/scheduler/fpu/mod.rs
//
// Module FPU du scheduler — séparation logique état / instructions ASM.
// arch/x86_64/cpu/fpu.rs = instructions brutes (XSAVE/XRSTOR/FXSAVE).
// Ce module = politique (lazy, quand sauvegarder, pour quel thread).

pub mod state;
pub mod lazy;
pub mod save_restore;

pub use state::{FpuState, XSAVE_AREA_SIZE, detect_xsave_size, FXSAVE_SIZE};
pub use lazy::{init, mark_fpu_not_loaded, handle_nm_exception, cr0_set_ts, cr0_clear_ts};
pub use save_restore::{xsave_current, xrstor_for, alloc_fpu_state};
