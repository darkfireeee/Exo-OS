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

pub fn init() {
    unsafe {
        // Enable SSE
        let mut cr0 = crate::arch::x86_64::registers::read_cr0();
        cr0 &= !(1 << 2); // Clear EM
        cr0 |= 1 << 1;    // Set MP
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack));

        let mut cr4 = crate::arch::x86_64::registers::read_cr4();
        cr4 |= 3 << 9;    // Set OSFXSR and OSXMMEXCPT
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
    }

    log::info!("SIMD initialized (SSE enabled)");
}
