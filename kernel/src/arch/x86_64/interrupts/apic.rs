//! Local APIC and x2APIC support

pub const APIC_BASE_MSR: u32 = 0x1B;

use core::arch::x86_64::__cpuid;
use alloc::format;

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

// APIC registers (MMIO offsets)
const APIC_ID: usize = 0x20;
const APIC_VERSION: usize = 0x30;
const APIC_TPR: usize = 0x80;  // Task Priority Register
const APIC_EOI: usize = 0xB0;  // End Of Interrupt
const APIC_SIVR: usize = 0xF0; // Spurious Interrupt Vector Register

// x2APIC MSRs
const IA32_APIC_BASE: u32 = 0x1B;
const X2APIC_APIC_ID: u32 = 0x802;
const X2APIC_EOI: u32 = 0x80B;
const X2APIC_SIVR: u32 = 0x80F;
// Timer MSRs (x2APIC)
const X2APIC_LVT_TIMER: u32 = 0x832;
const X2APIC_TIMER_ICR: u32 = 0x838;  // Initial Count Register
const X2APIC_TIMER_DCR: u32 = 0x83E;  // Divide Configuration Register

/// Local APIC structure
pub struct LocalApic {
    base_addr: usize,
    x2apic_mode: bool,
}

impl LocalApic {
    pub fn new(base_addr: usize) -> Self {
        Self { 
            base_addr,
            x2apic_mode: false,
        }
    }
    
    pub fn init(&mut self) {
        // FORCE xAPIC mode for SMP debugging (better QEMU compatibility)
        crate::logger::early_print("[APIC] FORCING xAPIC mode for SMP debugging...\n");
        log::info!("xAPIC mode enabled (MMIO at 0xFEE00000) - FORCED for SMP");
        self.init_xapic();
        self.x2apic_mode = false;
        
        // TODO: Re-enable x2APIC once SMP works:
        // if X2Apic::is_supported() {
        //     X2Apic::enable();
        //     self.x2apic_mode = true;
        // }
        
        // Enable APIC via spurious interrupt vector (works for both modes)
        self.set_spurious_interrupt_vector(0xFF);
        
        let apic_id = self.get_id();
        crate::logger::early_print(&format!("[APIC] ✓ APIC initialized - ID = {}\n", apic_id));
        log::info!("Local APIC initialized, ID = {}", apic_id);
    }
    
    fn init_xapic(&mut self) {
        crate::logger::early_print("[APIC] init_xapic called\n");
        // Enable APIC in APIC_BASE MSR
        unsafe {
            let mut apic_base = rdmsr(IA32_APIC_BASE);
            apic_base |= 1 << 11; // Enable bit
            wrmsr(IA32_APIC_BASE, apic_base);
        }
        
        // Set Task Priority to accept all interrupts
        self.write_reg(APIC_TPR, 0);
    }
    
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            let addr = (self.base_addr + offset) as *mut u32;
            core::ptr::write_volatile(addr, value);
        }
    }
    
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe {
            let addr = (self.base_addr + offset) as *const u32;
            core::ptr::read_volatile(addr)
        }
    }
    
    fn set_spurious_interrupt_vector(&mut self, vector: u8) {
        if self.x2apic_mode {
            unsafe {
                let value = (rdmsr(X2APIC_SIVR) & !0xFF) | (vector as u64) | (1 << 8);
                wrmsr(X2APIC_SIVR, value);
            }
        } else {
            let value = (vector as u32) | (1 << 8); // Bit 8 = APIC enable
            self.write_reg(APIC_SIVR, value);
        }
    }
    
    pub fn send_eoi(&self) {
        if self.x2apic_mode {
            unsafe {
                wrmsr(X2APIC_EOI, 0);
            }
        } else {
            self.write_reg(APIC_EOI, 0);
        }
    }
    
    pub fn get_id(&self) -> u32 {
        if self.x2apic_mode {
            unsafe { rdmsr(X2APIC_APIC_ID) as u32 }
        } else {
            self.read_reg(APIC_ID) >> 24
        }
    }
}

/// x2APIC support (MSR-based APIC)
pub struct X2Apic;

impl X2Apic {
    pub fn is_supported() -> bool {
        unsafe {
            let result = __cpuid(1);
            (result.ecx & (1 << 21)) != 0 // x2APIC bit
        }
    }
    
    pub fn enable() {
        unsafe {
            let mut apic_base = rdmsr(IA32_APIC_BASE);
            apic_base |= (1 << 11) | (1 << 10); // Enable + x2APIC mode
            wrmsr(IA32_APIC_BASE, apic_base);
        }
    }
}

/// Global Local APIC instance
pub static LOCAL_APIC: spin::Once<spin::Mutex<LocalApic>> = spin::Once::new();

/// Initialize Local APIC
pub fn init() {
    let apic = LOCAL_APIC.call_once(|| {
        let mut apic = LocalApic::new(0xFEE00000); // Default APIC base
        apic.init();
        spin::Mutex::new(apic)
    });
    
    log::info!("Local APIC initialized, ID = {}", apic.lock().get_id());
}

/// Send EOI to APIC
pub fn send_eoi() {
    if let Some(apic) = LOCAL_APIC.get() {
        apic.lock().send_eoi();
    }
}

/// Configure APIC Timer at 100Hz using xAPIC or x2APIC
/// This is called instead of PIT when using APIC mode
pub fn setup_timer(vector: u8) {
    crate::logger::early_print("[APIC] Setting up APIC Timer...\n");
    
    unsafe {
        // Check if x2APIC is enabled
        let apic_base = rdmsr(IA32_APIC_BASE);
        let x2apic_enabled = (apic_base & (1 << 10)) != 0;
        
        if x2apic_enabled {
            // Use x2APIC MSRs
            // Set divide value to 16 (value 3 = divide by 16)
            wrmsr(X2APIC_TIMER_DCR, 0x03);
            
            // Configure LVT Timer: periodic mode, vector number
            // Bit 17 = 1 for periodic, bits 0-7 = vector
            let lvt_timer = (1 << 17) | (vector as u64);
            wrmsr(X2APIC_LVT_TIMER, lvt_timer);
            
            // Set initial count for ~100Hz
            let initial_count: u64 = 62_500;
            wrmsr(X2APIC_TIMER_ICR, initial_count);
            
            crate::logger::early_print(&alloc::format!(
                "[APIC] ✓ APIC Timer configured (x2APIC MSR): vector={}, periodic, ICR={}\n", 
                vector, initial_count
            ));
        } else {
            // Use xAPIC MMIO
            const APIC_LVT_TIMER: usize = 0x320;
            const APIC_TIMER_DCR: usize = 0x3E0;
            const APIC_TIMER_ICR: usize = 0x380;
            const APIC_BASE_ADDR: usize = 0xFEE00000;
            
            let write_apic_reg = |offset: usize, value: u32| {
                let addr = (APIC_BASE_ADDR + offset) as *mut u32;
                core::ptr::write_volatile(addr, value);
            };
            
            // Set divide value to 16
            write_apic_reg(APIC_TIMER_DCR, 0x03);
            
            // Configure LVT Timer: periodic mode, vector number
            let lvt_timer = (1 << 17) | (vector as u32);
            write_apic_reg(APIC_LVT_TIMER, lvt_timer);
            
            // Set initial count for ~100Hz
            let initial_count: u32 = 62_500;
            write_apic_reg(APIC_TIMER_ICR, initial_count);
            
            crate::logger::early_print(&alloc::format!(
                "[APIC] ✓ APIC Timer configured (xAPIC MMIO): vector={}, periodic, ICR={}\n", 
                vector, initial_count
            ));
        }
    }
}
