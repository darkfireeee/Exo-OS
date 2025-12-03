//! CPU Power Management - DVFS (Dynamic Voltage and Frequency Scaling)
//! 
//! Implements P-states, C-states, and Intel Speed Shift / AMD Cool'n'Quiet.
//! Uses IA32_PERF_CTL MSR for frequency control.

use core::arch::x86_64::__cpuid;

/// MSR addresses for power management
mod msrs {
    pub const IA32_PERF_STATUS: u32 = 0x198;
    pub const IA32_PERF_CTL: u32 = 0x199;
    pub const IA32_MISC_ENABLE: u32 = 0x1A0;
    pub const MSR_PLATFORM_INFO: u32 = 0xCE;
    pub const MSR_TURBO_RATIO_LIMIT: u32 = 0x1AD;
    pub const IA32_PM_ENABLE: u32 = 0x770;      // HWP Enable
    pub const IA32_HWP_CAPABILITIES: u32 = 0x771;
    pub const IA32_HWP_REQUEST: u32 = 0x774;
    pub const IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;
}

/// C-states (CPU idle states)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CState {
    /// Active - CPU executing
    C0 = 0,
    /// Halt - Clock stopped, fast resume
    C1 = 1,
    /// Stop-Clock - Deeper sleep
    C2 = 2,
    /// Sleep - Caches flushed
    C3 = 3,
    /// Deep Sleep - Even lower power
    C6 = 6,
}

/// P-state (Performance state) information
#[derive(Debug, Clone, Copy)]
pub struct PState {
    /// Frequency ratio (multiplier)
    pub ratio: u8,
    /// Frequency in MHz
    pub frequency_mhz: u32,
    /// Estimated voltage (if available)
    pub voltage_mv: Option<u32>,
}

/// Power management capabilities
#[derive(Debug, Clone, Copy)]
pub struct PowerCaps {
    /// Supports Hardware P-states (Intel HWP / AMD CPPC)
    pub hwp_supported: bool,
    /// HWP is currently enabled
    pub hwp_enabled: bool,
    /// Supports EIST (Enhanced Intel SpeedStep)
    pub eist_supported: bool,
    /// Turbo boost supported
    pub turbo_supported: bool,
    /// Turbo boost enabled
    pub turbo_enabled: bool,
    /// Minimum frequency ratio
    pub min_ratio: u8,
    /// Maximum non-turbo ratio
    pub max_ratio: u8,
    /// Maximum turbo ratio
    pub turbo_ratio: u8,
    /// Base clock frequency (usually 100MHz)
    pub bus_clock_mhz: u32,
}

static mut POWER_CAPS: PowerCaps = PowerCaps {
    hwp_supported: false,
    hwp_enabled: false,
    eist_supported: false,
    turbo_supported: false,
    turbo_enabled: false,
    min_ratio: 8,
    max_ratio: 30,
    turbo_ratio: 35,
    bus_clock_mhz: 100,
};

/// Detect power management capabilities
pub fn detect_capabilities() -> PowerCaps {
    unsafe {
        let mut caps = PowerCaps {
            hwp_supported: false,
            hwp_enabled: false,
            eist_supported: false,
            turbo_supported: false,
            turbo_enabled: false,
            min_ratio: 8,
            max_ratio: 30,
            turbo_ratio: 35,
            bus_clock_mhz: 100,
        };
        
        // Check CPUID leaf 1 for EIST
        let r1 = __cpuid(1);
        caps.eist_supported = (r1.ecx & (1 << 7)) != 0;
        
        // Check CPUID leaf 6 for HWP and turbo
        let r6 = __cpuid(6);
        caps.hwp_supported = (r6.eax & (1 << 7)) != 0;
        caps.turbo_supported = (r6.eax & (1 << 1)) != 0;
        
        // Read platform info MSR for ratios
        let platform_info = rdmsr(msrs::MSR_PLATFORM_INFO);
        caps.max_ratio = ((platform_info >> 8) & 0xFF) as u8;
        caps.min_ratio = ((platform_info >> 40) & 0xFF) as u8;
        
        // Read turbo ratio limit
        if caps.turbo_supported {
            let turbo_limit = rdmsr(msrs::MSR_TURBO_RATIO_LIMIT);
            caps.turbo_ratio = (turbo_limit & 0xFF) as u8;
        }
        
        // Check if turbo is enabled
        let misc_enable = rdmsr(msrs::IA32_MISC_ENABLE);
        caps.turbo_enabled = (misc_enable & (1 << 38)) == 0; // Bit 38 = disable turbo
        
        // Check if HWP is enabled
        if caps.hwp_supported {
            let pm_enable = rdmsr(msrs::IA32_PM_ENABLE);
            caps.hwp_enabled = (pm_enable & 1) != 0;
        }
        
        caps
    }
}

/// Enter a C-state (idle state)
pub fn enter_cstate(state: CState) {
    unsafe {
        match state {
            CState::C0 => {
                // Active - do nothing
            }
            CState::C1 => {
                // HLT instruction - simple halt
                core::arch::asm!("hlt", options(nomem, nostack));
            }
            CState::C2 | CState::C3 | CState::C6 => {
                // Use MWAIT for deeper C-states if available
                // For now, fall back to HLT
                // TODO: Implement MWAIT-based C-states
                core::arch::asm!("hlt", options(nomem, nostack));
            }
        }
    }
}

/// Set CPU frequency using P-state ratio
pub fn set_frequency_ratio(ratio: u8) {
    unsafe {
        let caps = get_capabilities();
        
        // Clamp ratio to valid range
        let ratio = ratio.clamp(caps.min_ratio, caps.turbo_ratio);
        
        // If HWP is enabled, use HWP request
        if caps.hwp_enabled {
            set_hwp_target(ratio);
        } else {
            // Use IA32_PERF_CTL
            let perf_ctl = (ratio as u64) << 8;
            wrmsr(msrs::IA32_PERF_CTL, perf_ctl);
        }
        
        log::debug!("Power: Set frequency ratio to {} ({}MHz)", 
            ratio, ratio as u32 * caps.bus_clock_mhz);
    }
}

/// Set target frequency in MHz
pub fn set_frequency(freq_mhz: u32) {
    let caps = get_capabilities();
    let ratio = (freq_mhz / caps.bus_clock_mhz) as u8;
    set_frequency_ratio(ratio);
}

/// Get current frequency ratio
pub fn get_current_ratio() -> u8 {
    unsafe {
        let perf_status = rdmsr(msrs::IA32_PERF_STATUS);
        ((perf_status >> 8) & 0xFF) as u8
    }
}

/// Get current frequency in MHz
pub fn get_frequency() -> u32 {
    let caps = get_capabilities();
    get_current_ratio() as u32 * caps.bus_clock_mhz
}

/// Enable turbo boost
pub fn enable_turbo() {
    unsafe {
        let mut misc_enable = rdmsr(msrs::IA32_MISC_ENABLE);
        misc_enable &= !(1 << 38); // Clear bit 38 to enable turbo
        wrmsr(msrs::IA32_MISC_ENABLE, misc_enable);
        
        POWER_CAPS.turbo_enabled = true;
        log::info!("Power: Turbo boost enabled");
    }
}

/// Disable turbo boost
pub fn disable_turbo() {
    unsafe {
        let mut misc_enable = rdmsr(msrs::IA32_MISC_ENABLE);
        misc_enable |= 1 << 38; // Set bit 38 to disable turbo
        wrmsr(msrs::IA32_MISC_ENABLE, misc_enable);
        
        POWER_CAPS.turbo_enabled = false;
        log::info!("Power: Turbo boost disabled");
    }
}

/// Enable Hardware P-states (Intel HWP)
pub fn enable_hwp() {
    unsafe {
        let caps = get_capabilities();
        if !caps.hwp_supported {
            log::warn!("Power: HWP not supported");
            return;
        }
        
        // Enable HWP
        wrmsr(msrs::IA32_PM_ENABLE, 1);
        
        // Set HWP request for balanced performance
        let hwp_caps = rdmsr(msrs::IA32_HWP_CAPABILITIES);
        let min_perf = (hwp_caps & 0xFF) as u8;
        let max_perf = ((hwp_caps >> 8) & 0xFF) as u8;
        let desired = ((hwp_caps >> 16) & 0xFF) as u8;
        
        let hwp_request = (min_perf as u64) 
            | ((max_perf as u64) << 8)
            | ((desired as u64) << 16)
            | (0u64 << 24); // EPP = 0 (performance)
        
        wrmsr(msrs::IA32_HWP_REQUEST, hwp_request);
        
        POWER_CAPS.hwp_enabled = true;
        log::info!("Power: HWP enabled (min={}, max={}, desired={})", 
            min_perf, max_perf, desired);
    }
}

/// Set HWP target performance
fn set_hwp_target(ratio: u8) {
    unsafe {
        let hwp_caps = rdmsr(msrs::IA32_HWP_CAPABILITIES);
        let min_perf = (hwp_caps & 0xFF) as u8;
        let max_perf = ((hwp_caps >> 8) & 0xFF) as u8;
        
        let target = ratio.clamp(min_perf, max_perf);
        
        let hwp_request = (min_perf as u64)
            | ((target as u64) << 8)  // Max = target
            | ((target as u64) << 16) // Desired = target
            | (0u64 << 24);           // EPP = performance
        
        wrmsr(msrs::IA32_HWP_REQUEST, hwp_request);
    }
}

/// Set energy/performance bias (0=performance, 15=power saving)
pub fn set_energy_bias(bias: u8) {
    unsafe {
        let bias = bias.min(15) as u64;
        wrmsr(msrs::IA32_ENERGY_PERF_BIAS, bias);
        log::debug!("Power: Energy bias set to {}", bias);
    }
}

/// Get available P-states
pub fn get_pstates() -> alloc::vec::Vec<PState> {
    let caps = get_capabilities();
    let mut states = alloc::vec::Vec::new();
    
    // Generate P-states from min to turbo ratio
    for ratio in (caps.min_ratio..=caps.turbo_ratio).rev() {
        states.push(PState {
            ratio,
            frequency_mhz: ratio as u32 * caps.bus_clock_mhz,
            voltage_mv: None, // Would need ACPI _PSS to get voltage
        });
    }
    
    states
}

/// Get power capabilities
pub fn get_capabilities() -> PowerCaps {
    unsafe { POWER_CAPS }
}

/// Read MSR helper
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write MSR helper
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
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

/// Initialize power management
pub fn init() {
    let caps = detect_capabilities();
    unsafe { POWER_CAPS = caps; }
    
    log::info!("Power Management initialized:");
    log::info!("  EIST: {}", if caps.eist_supported { "supported" } else { "not supported" });
    log::info!("  HWP: {} ({})", 
        if caps.hwp_supported { "supported" } else { "not supported" },
        if caps.hwp_enabled { "enabled" } else { "disabled" });
    log::info!("  Turbo: {} ({})",
        if caps.turbo_supported { "supported" } else { "not supported" },
        if caps.turbo_enabled { "enabled" } else { "disabled" });
    log::info!("  Ratios: min={}, max={}, turbo={}",
        caps.min_ratio, caps.max_ratio, caps.turbo_ratio);
    log::info!("  Current: {}MHz (ratio {})", get_frequency(), get_current_ratio());
    
    // Enable HWP if available
    if caps.hwp_supported && !caps.hwp_enabled {
        enable_hwp();
    }
}
