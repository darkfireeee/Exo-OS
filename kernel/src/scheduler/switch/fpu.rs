//! FPU state save/restore

use core::arch::asm;

/// FPU state size (512 bytes for FXSAVE)
const FPU_STATE_SIZE: usize = 512;

/// FPU state buffer
#[repr(C, align(16))]
pub struct FpuState {
    data: [u8; FPU_STATE_SIZE],
}

impl FpuState {
    pub const fn new() -> Self {
        Self {
            data: [0; FPU_STATE_SIZE],
        }
    }
}

/// Save FPU state using FXSAVE
pub unsafe fn save_fpu_state(state: &mut FpuState) {
    asm!(
        "fxsave [{}]",
        in(reg) state.data.as_mut_ptr(),
        options(nostack)
    );
}

/// Restore FPU state using FXRSTOR
pub unsafe fn restore_fpu_state(state: &FpuState) {
    asm!(
        "fxrstor [{}]",
        in(reg) state.data.as_ptr(),
        options(nostack)
    );
}
