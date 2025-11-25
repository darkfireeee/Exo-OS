//! SIMD state save/restore (AVX/AVX512)

use core::arch::asm;

/// SIMD state size (for XSAVE with AVX)
const SIMD_STATE_SIZE: usize = 1024;

/// SIMD state buffer
#[repr(C, align(64))]
pub struct SimdState {
    data: [u8; SIMD_STATE_SIZE],
}

impl SimdState {
    pub const fn new() -> Self {
        Self {
            data: [0; SIMD_STATE_SIZE],
        }
    }
}

/// Save SIMD state using XSAVE
pub unsafe fn save_simd_state(state: &mut SimdState) {
    asm!(
        "xsave [{}]",
        in(reg) state.data.as_mut_ptr(),
        in("eax") 0xFFFFFFFFu32,
        in("edx") 0xFFFFFFFFu32,
        options(nostack)
    );
}

/// Restore SIMD state using XRSTOR
pub unsafe fn restore_simd_state(state: &SimdState) {
    asm!(
        "xrstor [{}]",
        in(reg) state.data.as_ptr(),
        in("eax") 0xFFFFFFFFu32,
        in("edx") 0xFFFFFFFFu32,
        options(nostack)
    );
}
