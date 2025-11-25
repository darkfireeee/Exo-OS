//! CPU Feature Detection and Management

use super::cpuid::{cpuid, has_feature};

/// CPU features cache
static mut FEATURES: CpuFeatures = CpuFeatures::empty();

#[derive(Debug, Clone, Copy)]
pub struct CpuFeatures {
    pub sse3: bool,
    pub ssse3: bool,
    pub sse4_1: bool,
    pub sse4_2: bool,
    pub avx: bool,
    pub avx2: bool,
    pub avx512f: bool,
    pub rdrand: bool,
    pub rdseed: bool,
    pub bmi1: bool,
    pub bmi2: bool,
    pub adx: bool,
    pub sha: bool,
    pub aes: bool,
}

impl CpuFeatures {
    const fn empty() -> Self {
        Self {
            sse3: false,
            ssse3: false,
            sse4_1: false,
            sse4_2: false,
            avx: false,
            avx2: false,
            avx512f: false,
            rdrand: false,
            rdseed: false,
            bmi1: false,
            bmi2: false,
            adx: false,
            sha: false,
            aes: false,
        }
    }
}

/// Detect all CPU features
pub fn detect() -> CpuFeatures {
    CpuFeatures {
        sse3: has_feature(1, 2, 0),      // ECX.SSE3
        ssse3: has_feature(1, 2, 9),     // ECX.SSSE3
        sse4_1: has_feature(1, 2, 19),   // ECX.SSE4.1
        sse4_2: has_feature(1, 2, 20),   // ECX.SSE4.2
        avx: has_feature(1, 2, 28),      // ECX.AVX
        avx2: has_feature(7, 1, 5),      // EBX.AVX2
        avx512f: has_feature(7, 1, 16),  // EBX.AVX512F
        rdrand: has_feature(1, 2, 30),   // ECX.RDRAND
        rdseed: has_feature(7, 1, 18),   // EBX.RDSEED
        bmi1: has_feature(7, 1, 3),      // EBX.BMI1
        bmi2: has_feature(7, 1, 8),      // EBX.BMI2
        adx: has_feature(7, 1, 19),      // EBX.ADX
        sha: has_feature(7, 1, 29),      // EBX.SHA
        aes: has_feature(1, 2, 25),      // ECX.AES
    }
}

/// Get cached features
pub fn get() -> CpuFeatures {
    unsafe { FEATURES }
}

/// Initialize feature detection
pub fn init() {
    let features = detect();
    unsafe { FEATURES = features; }
    
    log::info!("CPU Features:");
    log::info!("  SSE3: {}, SSSE3: {}, SSE4.1: {}, SSE4.2: {}", 
        features.sse3, features.ssse3, features.sse4_1, features.sse4_2);
    log::info!("  AVX: {}, AVX2: {}, AVX512F: {}", 
        features.avx, features.avx2, features.avx512f);
    log::info!("  AES: {}, SHA: {}", features.aes, features.sha);
}
