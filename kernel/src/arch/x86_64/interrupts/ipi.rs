//! IPI (Inter-Processor Interrupts) Support
//!
//! IPIs are used to signal other CPUs for:
//! - AP (Application Processor) startup (INIT + SIPI)
//! - Scheduler reschedule requests
//! - TLB shootdown (flush TLB on other CPUs)
//! - CPU halt/stop

use crate::arch::x86_64::smp::SMP_SYSTEM;
use core::arch::asm;
use core::sync::atomic::Ordering;

/// x2APIC MSR addresses
const IA32_APIC_BASE: u32 = 0x1B;
const X2APIC_ICR: u32 = 0x830; // Interrupt Command Register (64-bit in x2APIC)

/// xAPIC MMIO addresses (relative to 0xFEE00000)
const XAPIC_BASE: usize = 0xFEE00000;
const XAPIC_ICR_LOW: usize = 0x300;  // ICR bits 0-31
const XAPIC_ICR_HIGH: usize = 0x310; // ICR bits 32-63

/// IPI Vector assignments
pub const IPI_RESCHEDULE_VECTOR: u8 = 0xF0;
pub const IPI_TLB_FLUSH_VECTOR: u8 = 0xF1;
pub const IPI_HALT_VECTOR: u8 = 0xF2;

/// IPI delivery modes (bits 8-10 of ICR)
const DELIVERY_MODE_FIXED: u64 = 0b000 << 8;
const DELIVERY_MODE_INIT: u64 = 0b101 << 8;
const DELIVERY_MODE_STARTUP: u64 = 0b110 << 8;

/// IPI destination shorthand (bits 18-19 of ICR)
const DEST_SHORTHAND_NONE: u64 = 0b00 << 18;
const DEST_SHORTHAND_SELF: u64 = 0b01 << 18;
const DEST_SHORTHAND_ALL_INCLUDING_SELF: u64 = 0b10 << 18;
const DEST_SHORTHAND_ALL_EXCLUDING_SELF: u64 = 0b11 << 18;

/// ICR flags
const LEVEL_ASSERT: u64 = 1 << 14;
const LEVEL_DEASSERT: u64 = 0 << 14;
const TRIGGER_EDGE: u64 = 0 << 15;
const TRIGGER_LEVEL: u64 = 1 << 15;

/// Read MSR
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write MSR
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );
}

/// Write to xAPIC register (MMIO)
#[inline]
unsafe fn write_xapic_reg(offset: usize, value: u32) {
    let addr = (XAPIC_BASE + offset) as *mut u32;
    core::ptr::write_volatile(addr, value);
}

/// Read from xAPIC register (MMIO)
#[inline]
unsafe fn read_xapic_reg(offset: usize) -> u32 {
    let addr = (XAPIC_BASE + offset) as *const u32;
    core::ptr::read_volatile(addr)
}

/// Check if we should use xAPIC mode (force for SMP debugging)
#[inline]
fn use_xapic_mode() -> bool {
    // FORCE xAPIC for SMP debugging (QEMU compatibility)
    true
    // TODO: Change to `!is_x2apic_enabled()` once SMP works
}

/// Send INIT IPI to a specific APIC ID
///
/// INIT IPI resets the target CPU to its initial state (real mode, CS=F000h, IP=FFF0h).
/// This is the first step in AP (Application Processor) startup.
pub fn send_init_ipi(apic_id: u32) {
    log::info!("[IPI] send_init_ipi() ENTERED for APIC {}", apic_id);
    
    unsafe {
        let icr_low = DELIVERY_MODE_INIT
            | LEVEL_ASSERT
            | TRIGGER_LEVEL
            | DEST_SHORTHAND_NONE;
        
        if use_xapic_mode() {
            log::info!("[IPI] Using xAPIC (MMIO) mode");
            log::info!("[IPI] Writing ICR_HIGH = {:#010x} (dest = {})", apic_id << 24, apic_id);
            write_xapic_reg(XAPIC_ICR_HIGH, apic_id << 24);  // Destination in bits 24-31
            
            log::info!("[IPI] Writing ICR_LOW = {:#010x}", icr_low as u32);
            write_xapic_reg(XAPIC_ICR_LOW, icr_low as u32);
        } else {
            let icr_value = icr_low | ((apic_id as u64) << 32);
            log::info!("[IPI] Using x2APIC (MSR) mode");
            log::info!("[IPI] Writing ICR value {:#018x} to MSR {:#x}", icr_value, X2APIC_ICR);
            wrmsr(X2APIC_ICR, icr_value);
        }
        
        log::info!("[IPI] INIT IPI sent successfully");
    }
}

/// Send SIPI (Startup IPI) to a specific APIC ID
///
/// SIPI starts the target CPU executing at physical address (vector * 4096).
/// The vector is typically 0x08 for address 0x8000.
pub fn send_startup_ipi(apic_id: u32, vector: u8) {
    log::info!("[IPI] send_startup_ipi() ENTERED for APIC {}, vector {:#x}", apic_id, vector);
    
    unsafe {
        let icr_low = (vector as u64)
            | DELIVERY_MODE_STARTUP
            | LEVEL_ASSERT
            | DEST_SHORTHAND_NONE;
        
        if use_xapic_mode() {
            log::info!("[IPI] Using xAPIC (MMIO) mode for SIPI");
            log::info!("[IPI] Writing ICR_HIGH = {:#010x} (dest = {})", apic_id << 24, apic_id);
            write_xapic_reg(XAPIC_ICR_HIGH, apic_id << 24);
            
            log::info!("[IPI] Writing ICR_LOW = {:#010x} (vector={:#x}, addr={:#x})", icr_low as u32, vector, (vector as u32) * 0x1000);
            write_xapic_reg(XAPIC_ICR_LOW, icr_low as u32);
        } else {
            let icr_value = icr_low | ((apic_id as u64) << 32);
            log::info!("[IPI] Using x2APIC (MSR) mode for SIPI");
            log::info!("[IPI] Writing SIPI ICR value {:#018x} to MSR {:#x}", icr_value, X2APIC_ICR);
            wrmsr(X2APIC_ICR, icr_value);
        }
        
        log::info!("[IPI] SIPI sent successfully");
    }
}

/// Send a fixed IPI to a specific APIC ID
pub fn send_ipi(apic_id: u32, vector: u8) {
    unsafe {
        let icr_value = (vector as u64)
            | DELIVERY_MODE_FIXED
            | DEST_SHORTHAND_NONE
            | ((apic_id as u64) << 32);
        
        wrmsr(X2APIC_ICR, icr_value);
    }
}

/// Send reschedule IPI to a specific CPU
pub fn send_reschedule_ipi(cpu_id: usize) {
    if let Some(cpu) = SMP_SYSTEM.cpu(cpu_id) {
        if cpu.is_online() {
            send_ipi(cpu.apic_id.load(Ordering::Acquire) as u32, IPI_RESCHEDULE_VECTOR);
        }
    }
}

/// Send TLB flush IPI to a specific CPU
pub fn send_tlb_flush_ipi(cpu_id: usize) {
    if let Some(cpu) = SMP_SYSTEM.cpu(cpu_id) {
        if cpu.is_online() {
            send_ipi(cpu.apic_id.load(Ordering::Acquire) as u32, IPI_TLB_FLUSH_VECTOR);
        }
    }
}

/// Send halt IPI to a specific CPU
pub fn send_halt_ipi(cpu_id: usize) {
    if let Some(cpu) = SMP_SYSTEM.cpu(cpu_id) {
        if cpu.is_online() {
            send_ipi(cpu.apic_id.load(Ordering::Acquire) as u32, IPI_HALT_VECTOR);
        }
    }
}

/// Send IPI to all CPUs except self
pub fn send_ipi_all_but_self(vector: u8) {
    unsafe {
        let icr_value = (vector as u64)
            | DELIVERY_MODE_FIXED
            | DEST_SHORTHAND_ALL_EXCLUDING_SELF;
        
        wrmsr(X2APIC_ICR, icr_value);
    }
}

/// Send reschedule IPI to all CPUs except self
pub fn send_reschedule_all_but_self() {
    send_ipi_all_but_self(IPI_RESCHEDULE_VECTOR);
}

/// Send TLB flush IPI to all CPUs except self
pub fn send_tlb_flush_all_but_self() {
    send_ipi_all_but_self(IPI_TLB_FLUSH_VECTOR);
}

/// Wait for ICR to be idle (delivery status clear)
///
/// Note: In x2APIC mode, the delivery status is always 0 (idle),
/// so this function is a no-op for compatibility.
pub fn wait_for_ipi_idle() {
    // In x2APIC mode, writes to ICR are guaranteed to be accepted immediately
    // No need to poll for delivery status
}

/// Check if x2APIC is enabled
pub fn is_x2apic_enabled() -> bool {
    unsafe {
        let apic_base = rdmsr(IA32_APIC_BASE);
        (apic_base & (1 << 10)) != 0 // Bit 10 = x2APIC enable
    }
}
