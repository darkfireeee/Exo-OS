//! Model-Specific Registers (MSR)
//! 
//! Read/write CPU-specific registers.

/// Read MSR
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (high, low): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write MSR
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );
}

/// Common MSRs
pub mod msrs {
    pub const IA32_APIC_BASE: u32 = 0x1B;
    pub const IA32_EFER: u32 = 0xC0000080;
    pub const IA32_STAR: u32 = 0xC0000081;
    pub const IA32_LSTAR: u32 = 0xC0000082;
    pub const IA32_FMASK: u32 = 0xC0000084;
    pub const IA32_FS_BASE: u32 = 0xC0000100;
    pub const IA32_GS_BASE: u32 = 0xC0000101;
    pub const IA32_KERNEL_GS_BASE: u32 = 0xC0000102;
    pub const IA32_TSC_AUX: u32 = 0xC0000103;
}

pub fn init() {
    // MSRs initialized per-CPU
}
