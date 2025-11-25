//! HPET (High Precision Event Timer) driver
//! 
//! Provides high-resolution hardware timer

use core::ptr::{read_volatile, write_volatile};

/// HPET MMIO base address (from ACPI tables)
static mut HPET_BASE: *mut u8 = core::ptr::null_mut();

/// HPET register offsets
const HPET_CAP_ID: usize = 0x00;
const HPET_CONFIG: usize = 0x10;
const HPET_COUNTER: usize = 0xF0;

/// HPET configuration bits
const HPET_ENABLE: u64 = 1 << 0;
const HPET_LEG_RT: u64 = 1 << 1;

/// HPET structure
pub struct Hpet {
    base: *mut u8,
    frequency: u64,
    period_fs: u64,
}

impl Hpet {
    /// Create HPET instance from MMIO base address
    pub unsafe fn new(base: *mut u8) -> Option<Self> {
        if base.is_null() {
            return None;
        }
        
        let cap_id = read_volatile((base as usize + HPET_CAP_ID) as *const u64);
        
        // Extract period (in femtoseconds)
        let period_fs = cap_id >> 32;
        
        // Calculate frequency (Hz)
        let frequency = 1_000_000_000_000_000 / period_fs;
        
        Some(Self {
            base,
            frequency,
            period_fs,
        })
    }
    
    /// Enable HPET
    pub fn enable(&mut self) {
        unsafe {
            let config_addr = (self.base as usize + HPET_CONFIG) as *mut u64;
            let mut config = read_volatile(config_addr);
            config |= HPET_ENABLE;
            write_volatile(config_addr, config);
        }
    }
    
    /// Disable HPET
    pub fn disable(&mut self) {
        unsafe {
            let config_addr = (self.base as usize + HPET_CONFIG) as *mut u64;
            let mut config = read_volatile(config_addr);
            config &= !HPET_ENABLE;
            write_volatile(config_addr, config);
        }
    }
    
    /// Read main counter value
    pub fn read_counter(&self) -> u64 {
        unsafe {
            read_volatile((self.base as usize + HPET_COUNTER) as *const u64)
        }
    }
    
    /// Get HPET frequency in Hz
    pub fn frequency(&self) -> u64 {
        self.frequency
    }
    
    /// Convert counter ticks to nanoseconds
    pub fn ticks_to_ns(&self, ticks: u64) -> u64 {
        // ns = ticks * period_fs / 1_000_000
        ticks.saturating_mul(self.period_fs) / 1_000_000
    }
    
    /// Convert nanoseconds to counter ticks
    pub fn ns_to_ticks(&self, ns: u64) -> u64 {
        // ticks = ns * 1_000_000 / period_fs
        ns.saturating_mul(1_000_000) / self.period_fs
    }
}

/// Global HPET instance
static mut HPET_INSTANCE: Option<Hpet> = None;

/// Initialize HPET
pub fn init() -> Result<(), &'static str> {
    // TODO: Get HPET base address from ACPI HPET table
    // For now, use common default address (0xFED00000)
    let base = 0xFED00000 as *mut u8;
    
    unsafe {
        if let Some(mut hpet) = Hpet::new(base) {
            hpet.enable();
            HPET_BASE = base;
            HPET_INSTANCE = Some(hpet);
            Ok(())
        } else {
            Err("HPET not available")
        }
    }
}

/// Get HPET instance
pub fn get() -> Option<&'static Hpet> {
    unsafe { HPET_INSTANCE.as_ref() }
}

/// Read HPET counter
pub fn read_counter() -> u64 {
    get().map(|h| h.read_counter()).unwrap_or(0)
}

/// Convert ticks to nanoseconds
pub fn ticks_to_ns(ticks: u64) -> u64 {
    get().map(|h| h.ticks_to_ns(ticks)).unwrap_or(0)
}

/// Initialize HPET (shorthand)
pub fn init_hpet() -> Result<(), &'static str> {
    init()
}
