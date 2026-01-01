//! SIMD State Management
//! 
//! FPU/SSE/AVX state save/restore (lazy evaluation).

#[repr(C, align(64))]
pub struct FxsaveArea {
    data: [u8; 512],
}

impl FxsaveArea {
    pub const fn new() -> Self {
        FxsaveArea { data: [0; 512] }
    }

    pub fn save(&mut self) {
        unsafe {
            core::arch::asm!(
                "fxsave64 [{}]",
                in(reg) self.data.as_mut_ptr(),
                options(nostack)
            );
        }
    }

    pub fn restore(&self) {
        unsafe {
            core::arch::asm!(
                "fxrstor64 [{}]",
                in(reg) self.data.as_ptr(),
                options(nostack)
            );
        }
    }
}

/// Early SSE initialization - call BEFORE any code that might use SSE instructions
/// This includes log macros, format!, and potentially memory copies.
/// Does NOT use any logging since the logger might use SSE itself.
#[inline(always)]
pub fn init_early() {
    unsafe {
        // Enable SSE in CR0: Clear EM (bit 2), Set MP (bit 1)
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1 << 2); // Clear EM (Emulation)
        cr0 |= 1 << 1;    // Set MP (Monitor Coprocessor)
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack));

        // Enable SSE in CR4: Set OSFXSR (bit 9) and OSXMMEXCPT (bit 10)
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= 3 << 9;    // Set OSFXSR and OSXMMEXCPT
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
    }
}

/// Full SIMD initialization with logging
pub fn init() {
    // SSE should already be enabled by init_early()
    // This just logs the status
    log::info!("SIMD initialized (SSE enabled)");
}
