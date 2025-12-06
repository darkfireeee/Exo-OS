//! AP (Application Processor) Bootstrap
//!
//! Handles copying trampoline code to low memory and setting up AP startup.

use core::arch::asm;
use core::ptr;

/// Trampoline location in low memory (< 1MB for real mode access)
pub const TRAMPOLINE_ADDR: usize = 0x8000; // 32KB mark

/// External symbols from trampoline.asm
extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_trampoline_size: usize;
}

/// Get current CR3 (PML4 physical address)
fn read_cr3() -> u64 {
    let cr3: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    cr3
}

/// Setup trampoline code for AP startup
///
/// # Arguments
/// * `cpu_id` - The CPU ID that will be passed to ap_startup()
/// * `stack_top` - The top of the stack for the AP (must be unique per AP)
/// * `entry_point` - The ap_startup() function pointer
///
/// # Returns
/// * `Ok(vector)` - SIPI vector to send (address / 4096)
/// * `Err(&str)` - Error message
pub fn setup_trampoline(cpu_id: usize, stack_top: u64, entry_point: u64) -> Result<u8, &'static str> {
    unsafe {
        // 1. Get trampoline code size
        let trampoline_start = &ap_trampoline_start as *const u8;
        let trampoline_end = &ap_trampoline_end as *const u8;
        let trampoline_size = trampoline_end as usize - trampoline_start as usize;
        
        if trampoline_size == 0 || trampoline_size > 0x1000 {
            log::error!("Invalid trampoline size: {:#x}", trampoline_size);
            return Err("Invalid trampoline size");
        }
        
        log::debug!("Trampoline size: {:#x} bytes", trampoline_size);
        
        // 2. Copy trampoline code to low memory
        let src = trampoline_start;
        let dst = TRAMPOLINE_ADDR as *mut u8;
        
        ptr::copy_nonoverlapping(src, dst, trampoline_size);
        
        log::debug!("Copied trampoline to {:#x}", TRAMPOLINE_ADDR);
        
        // 3. Setup data variables in trampoline
        // Offsets match trampoline.asm data section
        
        // PML4 address (0x8200)
        let pml4_addr = read_cr3();
        let pml4_ptr = (TRAMPOLINE_ADDR + 0x200) as *mut u64;
        ptr::write_volatile(pml4_ptr, pml4_addr);
        
        // Stack pointer (0x8208)
        let stack_ptr = (TRAMPOLINE_ADDR + 0x208) as *mut u64;
        ptr::write_volatile(stack_ptr, stack_top);
        
        // CPU ID (0x8210)
        let cpu_id_ptr = (TRAMPOLINE_ADDR + 0x210) as *mut u64;
        ptr::write_volatile(cpu_id_ptr, cpu_id as u64);
        
        // Entry point (0x8218)
        let entry_ptr = (TRAMPOLINE_ADDR + 0x218) as *mut u64;
        ptr::write_volatile(entry_ptr, entry_point);
        
        log::info!(
            "AP {} trampoline ready: PML4={:#x}, Stack={:#x}, Entry={:#x}",
            cpu_id, pml4_addr, stack_top, entry_point
        );
        
        // 4. Calculate SIPI vector (physical address / 4096)
        let vector = (TRAMPOLINE_ADDR / 0x1000) as u8;
        
        Ok(vector)
    }
}

/// Allocate a stack for an AP
///
/// Returns the top of the stack (stack grows downward)
pub fn allocate_ap_stack(cpu_id: usize) -> Result<u64, &'static str> {
    use alloc::vec::Vec;
    
    const AP_STACK_SIZE: usize = 64 * 1024; // 64KB per AP
    
    // Allocate stack on heap
    let mut stack = Vec::<u8>::with_capacity(AP_STACK_SIZE);
    stack.resize(AP_STACK_SIZE, 0);
    
    // Leak the Vec to keep it alive permanently
    let stack_ptr = stack.as_mut_ptr();
    core::mem::forget(stack);
    
    // Stack top is at end of allocation (grows downward)
    let stack_top = unsafe { stack_ptr.add(AP_STACK_SIZE) } as u64;
    
    log::debug!("Allocated AP {} stack at {:#x}", cpu_id, stack_top);
    
    Ok(stack_top)
}
