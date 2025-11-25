//! Symmetric Multi-Processing (SMP)
//! 
//! Boot and manage multiple CPU cores.

use super::msr::{rdmsr, wrmsr, msrs};
use core::sync::atomic::{AtomicU32, Ordering};

static AP_COUNT: AtomicU32 = AtomicU32::new(0);
static BSP_ID: AtomicU32 = AtomicU32::new(0);

/// Boot Application Processors (APs)
pub fn boot_aps() {
    // TODO: Implement AP boot
    // 1. Send INIT IPI
    // 2. Send SIPI with trampoline address
    // 3. Wait for AP to signal ready
    log::info!("SMP: BSP booted, APs initialization skipped (TODO)");
}

/// Called by each AP during boot
pub unsafe fn ap_init(ap_id: u32) {
    // Setup per-CPU data
    // - GDT, IDT
    // - Local APIC
    // - Enable features
    
    AP_COUNT.fetch_add(1, Ordering::SeqCst);
    log::info!("AP {} initialized", ap_id);
}

/// Get number of active CPUs
pub fn cpu_count() -> u32 {
    1 + AP_COUNT.load(Ordering::Relaxed)
}

/// Get current CPU ID
pub fn current_cpu() -> u32 {
    // TODO: Read from local APIC ID or per-CPU variable
    unsafe {
        let tsc_aux = rdmsr(msrs::IA32_TSC_AUX);
        (tsc_aux & 0xFFF) as u32
    }
}

/// Send IPI (Inter-Processor Interrupt)
pub fn send_ipi(target_cpu: u32, vector: u8) {
    // TODO: Program local APIC ICR
}

pub fn init() {
    log::info!("SMP: Initializing (BSP only for now)");
    BSP_ID.store(0, Ordering::SeqCst);
}
