//! AP (Application Processor) Bootstrap
//!
//! Handles copying trampoline code to low memory and setting up AP startup.

use core::arch::asm;
use core::ptr;

/// Trampoline location in low memory (< 1MB for real mode access)
pub const TRAMPOLINE_ADDR: usize = 0x8000; // 32KB mark

/// Offsets de données après le code du trampoline (aligné sur 4KB pour sécurité)
/// Le code du trampoline fait ~240 bytes, donc on place les données à 0x1000 (4KB)
/// Cela évite tout conflit et assure un accès mémoire propre
const DATA_OFFSET: usize = 0x1000;                  // Début à 0x9000 (4KB après 0x8000)
const PML4_OFFSET: usize = DATA_OFFSET + 0x00;      // 0x9000
const STACK_OFFSET: usize = DATA_OFFSET + 0x08;     // 0x9008  
const CPU_ID_OFFSET: usize = DATA_OFFSET + 0x10;    // 0x9010
const ENTRY_OFFSET: usize = DATA_OFFSET + 0x18;     // 0x9018
const GDT_PTR_OFFSET: usize = DATA_OFFSET + 0x20;   // 0x9020 (10 bytes)
const IDT_PTR_OFFSET: usize = DATA_OFFSET + 0x2a;   // 0x902a (10 bytes)

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
        
        log::info!("Trampoline symbols: start={:p}, end={:p}", trampoline_start, trampoline_end);
        
        let trampoline_size = trampoline_end as usize - trampoline_start as usize;
        
        log::info!("Trampoline size calculated: {:#x} bytes ({})", trampoline_size, trampoline_size);
        
        if trampoline_size == 0 {
            log::error!("Trampoline size is ZERO - symbols not linked correctly!");
            log::error!("This means ap_trampoline.o was not linked into the kernel");
            return Err("Trampoline symbols not linked");
        }
        
        if trampoline_size > 0x1000 {
            log::error!("Invalid trampoline size: {:#x} (max 4KB)", trampoline_size);
            return Err("Trampoline too large");
        }
        
        log::debug!("Trampoline size: {:#x} bytes", trampoline_size);
        
        // 2. Copy trampoline code to low memory
        let src = trampoline_start;
        let dst = TRAMPOLINE_ADDR as *mut u8;
        
        ptr::copy_nonoverlapping(src, dst, trampoline_size);
        
        log::debug!("Copied trampoline to {:#x}", TRAMPOLINE_ADDR);
        
        // 3. Setup data variables in trampoline
        // Utilise des offsets relatifs pour une approche production-ready
        // Les données sont placées juste après le code du trampoline
        
        log::info!("Writing trampoline data at TRAMPOLINE_ADDR + DATA_OFFSET = {:#x}", TRAMPOLINE_ADDR + DATA_OFFSET);
        
        // Test magic value pour debugging
        let test_ptr = (TRAMPOLINE_ADDR + DATA_OFFSET - 8) as *mut u64;
        ptr::write_volatile(test_ptr, 0xDEADBEEFCAFEBABE);
        let test_readback = ptr::read_volatile(test_ptr);
        log::info!("Test magic @ {:#x} = {:#x}", TRAMPOLINE_ADDR + DATA_OFFSET - 8, test_readback);
        
        // PML4 address
        let pml4_addr = read_cr3();
        let pml4_ptr = (TRAMPOLINE_ADDR + PML4_OFFSET) as *mut u64;
        ptr::write_volatile(pml4_ptr, pml4_addr);
        
        // Vérification immédiate
        let pml4_readback = ptr::read_volatile(pml4_ptr);
        log::info!("PML4 @ {:#x} = {:#x} (readback: {:#x})", 
                   TRAMPOLINE_ADDR + PML4_OFFSET, pml4_addr, pml4_readback);
        
        if pml4_readback != pml4_addr {
            log::error!("PML4 verification FAILED! Written {:#x} but read back {:#x}", 
                        pml4_addr, pml4_readback);
            return Err("PML4 write verification failed");
        }
        
        // Stack pointer
        let stack_ptr = (TRAMPOLINE_ADDR + STACK_OFFSET) as *mut u64;
        ptr::write_volatile(stack_ptr, stack_top);
        log::info!("Stack @ {:#x} = {:#x}", TRAMPOLINE_ADDR + STACK_OFFSET, stack_top);
        
        // CPU ID
        let cpu_id_ptr = (TRAMPOLINE_ADDR + CPU_ID_OFFSET) as *mut u64;
        ptr::write_volatile(cpu_id_ptr, cpu_id as u64);
        log::info!("CPU_ID @ {:#x} = {}", TRAMPOLINE_ADDR + CPU_ID_OFFSET, cpu_id);
        
        // Entry point
        let entry_ptr = (TRAMPOLINE_ADDR + ENTRY_OFFSET) as *mut u64;
        ptr::write_volatile(entry_ptr, entry_point);
        log::info!("Entry @ {:#x} = {:#x}", TRAMPOLINE_ADDR + ENTRY_OFFSET, entry_point);
        
        // GDT64 pointer - 10 bytes: limit (2) + base (8)
        let (gdt_base, gdt_limit) = crate::arch::x86_64::gdt::get_gdt_info();
        let gdt_limit_ptr = (TRAMPOLINE_ADDR + GDT_PTR_OFFSET) as *mut u16;
        let gdt_base_ptr = (TRAMPOLINE_ADDR + GDT_PTR_OFFSET + 2) as *mut u64;
        ptr::write_volatile(gdt_limit_ptr, gdt_limit);
        ptr::write_volatile(gdt_base_ptr, gdt_base);
        log::info!("GDT @ {:#x}: limit={:#x}, base={:#x}", TRAMPOLINE_ADDR + GDT_PTR_OFFSET, gdt_limit, gdt_base);
        
        // IDT pointer - 10 bytes: limit (2) + base (8)
        let (idt_base, idt_limit) = crate::arch::x86_64::idt::get_idt_info();
        let idt_limit_ptr = (TRAMPOLINE_ADDR + IDT_PTR_OFFSET) as *mut u16;
        let idt_base_ptr = (TRAMPOLINE_ADDR + IDT_PTR_OFFSET + 2) as *mut u64;
        ptr::write_volatile(idt_limit_ptr, idt_limit);
        ptr::write_volatile(idt_base_ptr, idt_base);
        log::info!("IDT @ {:#x}: limit={:#x}, base={:#x}", TRAMPOLINE_ADDR + IDT_PTR_OFFSET, idt_limit, idt_base);
        
        log::info!("BEFORE multi-arg log");
        
        log::info!(
            "AP {} trampoline ready: PML4={:#x}, Stack={:#x}, Entry={:#x}",
            cpu_id, pml4_addr, stack_top, entry_point
        );
        
        log::info!("AFTER multi-arg log");
        log::info!("[DEBUG] About to calculate vector...");
        
        // 4. Calculate SIPI vector (physical address / 4096)
        let vector = (TRAMPOLINE_ADDR / 0x1000) as u8;
        
        log::info!("[DEBUG] Vector calculated: {:#x}", vector);
        log::info!("[DEBUG] Returning Ok(vector)...");
        
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
