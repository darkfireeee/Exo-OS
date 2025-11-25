//! CPUID Feature Detection
//! 
//! Parse CPU capabilities and features.

use core::arch::x86_64::__cpuid;

/// CPUID result
#[derive(Debug, Clone, Copy)]
pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// Execute CPUID instruction
pub fn cpuid(leaf: u32) -> CpuidResult {
    unsafe {
        let result = __cpuid(leaf);
        CpuidResult {
            eax: result.eax,
            ebx: result.ebx,
            ecx: result.ecx,
            edx: result.edx,
        }
    }
}

/// Get CPU vendor string
pub fn vendor() -> [u8; 12] {
    let r = cpuid(0);
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&r.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&r.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&r.ecx.to_le_bytes());
    vendor
}

/// Get CPU brand string
pub fn brand() -> [u8; 48] {
    let mut brand = [0u8; 48];
    for i in 0..3 {
        let r = cpuid(0x80000002 + i);
        let offset = i as usize * 16;
        brand[offset..offset+4].copy_from_slice(&r.eax.to_le_bytes());
        brand[offset+4..offset+8].copy_from_slice(&r.ebx.to_le_bytes());
        brand[offset+8..offset+12].copy_from_slice(&r.ecx.to_le_bytes());
        brand[offset+12..offset+16].copy_from_slice(&r.edx.to_le_bytes());
    }
    brand
}

/// Check if feature is supported
pub fn has_feature(leaf: u32, register: u8, bit: u32) -> bool {
    let r = cpuid(leaf);
    let value = match register {
        0 => r.eax,
        1 => r.ebx,
        2 => r.ecx,
        3 => r.edx,
        _ => return false,
    };
    (value & (1 << bit)) != 0
}

/// Feature flags
pub mod features {
    pub const SSE3: (u32, u8, u32) = (1, 2, 0);     // ECX bit 0
    pub const SSSE3: (u32, u8, u32) = (1, 2, 9);    // ECX bit 9
    pub const SSE4_1: (u32, u8, u32) = (1, 2, 19);  // ECX bit 19
    pub const SSE4_2: (u32, u8, u32) = (1, 2, 20);  // ECX bit 20
    pub const AVX: (u32, u8, u32) = (1, 2, 28);     // ECX bit 28
    pub const AVX2: (u32, u8, u32) = (7, 1, 5);     // EBX bit 5
    pub const AVX512F: (u32, u8, u32) = (7, 1, 16); // EBX bit 16
}

pub fn init() {
    let vendor = vendor();
    let brand = brand();
    log::info!("CPU Vendor: {}", core::str::from_utf8(&vendor).unwrap_or("Unknown"));
    log::info!("CPU Brand: {}", core::str::from_utf8(&brand).unwrap_or("Unknown"));
}
