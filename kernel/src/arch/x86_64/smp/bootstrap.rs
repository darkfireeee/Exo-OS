// ============================================================================
// AP Bootstrap Manager - Production-Ready SMP Initialization
// ============================================================================
//
// This module manages the setup and execution of the Application Processor
// bootstrap sequence. It prepares the trampoline code and data structures
// required for APs to transition from real mode to long mode.
//
// ARCHITECTURE:
// - BSP (Bootstrap Processor) prepares trampoline at 0x8000
// - BSP writes boot data to 0x8100
// - BSP sends INIT-SIPI-SIPI sequence to wake AP
// - AP executes trampoline code, transitions to long mode, calls Rust
//
// MEMORY LAYOUT:
//   0x8000-0x80FF: Trampoline code (256 bytes max)
//   0x8100-0x81FF: Boot data structure (256 bytes reserved)
//
// OPTIMIZATION:
// - Minimal data copying
// - Clear error handling
// - Aligned structures
// - Production-ready logging
// ============================================================================

use core::ptr;

/// Physical address where trampoline code is copied (must be < 1MB for real mode)
const TRAMPOLINE_CODE_ADDR: usize = 0x8000;

/// Physical address where boot data is written (512 bytes after code)
const TRAMPOLINE_DATA_ADDR: usize = 0x8200;

/// Boot data structure offsets (relative to TRAMPOLINE_DATA_ADDR)
mod data_offsets {
    pub const PML4: usize = 0x00;       // u64: Page table root
    pub const STACK: usize = 0x08;      // u64: Kernel stack pointer
    pub const CPU_ID: usize = 0x10;     // u64: Logical CPU ID
    pub const ENTRY: usize = 0x18;      // u64: Kernel entry point
    pub const GDT_DESC: usize = 0x20;   // 10 bytes: u16 limit + u64 base
    pub const IDT_DESC: usize = 0x2a;   // 10 bytes: u16 limit + u64 base
}

/// Setup the AP trampoline code and data structures
///
/// # Arguments
/// * `cpu_id` - Logical CPU ID for the AP
/// * `stack_top` - Virtual address of the kernel stack top for this AP
/// * `entry_point` - Virtual address of the Rust AP entry function
///
/// # Returns
/// SIPI vector (physical address / 4096) or error message
///
/// # Safety
/// This function writes to low physical memory. Must be called with interrupts
/// disabled and only once per AP boot.
pub fn setup_trampoline(
    cpu_id: usize,
    stack_top: u64,
    entry_point: u64,
) -> Result<u8, &'static str> {
    unsafe {
        // ====================================================================
        // STEP 1: Copy trampoline code to low memory
        // ====================================================================
        
        extern "C" {
            static ap_trampoline_start: u8;
            static ap_trampoline_end: u8;
        }
        
        let trampoline_start = &ap_trampoline_start as *const u8;
        let trampoline_end = &ap_trampoline_end as *const u8;
        let trampoline_size = trampoline_end as usize - trampoline_start as usize;
        
        log::debug!("AP Trampoline: code @ {:p}, size = {} bytes", trampoline_start, trampoline_size);
        
        if trampoline_size == 0 {
            log::error!("Trampoline size is zero - linking error!");
            return Err("Trampoline not linked");
        }
        
        if trampoline_size > 256 {
            log::error!("Trampoline too large: {} bytes (max 256)", trampoline_size);
            return Err("Trampoline too large");
        }
        
        // Copy trampoline code
        ptr::copy_nonoverlapping(
            trampoline_start,
            TRAMPOLINE_CODE_ADDR as *mut u8,
            trampoline_size,
        );
        
        log::debug!("Trampoline code copied to {:#x}", TRAMPOLINE_CODE_ADDR);
        
        // ====================================================================
        // STEP 2: Prepare boot data structure
        // ====================================================================
        
        // Get current page table root (PML4)
        let pml4_addr = read_cr3();
        
        // Get GDT information
        let (gdt_base, gdt_limit) = crate::arch::x86_64::gdt::get_gdt_info();
        
        // Get IDT information
        let (idt_base, idt_limit) = crate::arch::x86_64::idt::get_idt_info();
        
        log::info!(
            "AP {} boot data: PML4={:#x}, Stack={:#x}, Entry={:#x}",
            cpu_id, pml4_addr, stack_top, entry_point
        );
        log::debug!(
            "  System tables: GDT={:#x} (limit {}), IDT={:#x} (limit {})",
            gdt_base, gdt_limit, idt_base, idt_limit
        );
        
        // ====================================================================
        // STEP 3: Write boot data to memory
        // ====================================================================
        
        // DIAGNOSTIC: Write magic signatures around the PML4 value
        let magic_before_ptr = (TRAMPOLINE_DATA_ADDR - 8) as *mut u64;
        let magic_after_ptr = (TRAMPOLINE_DATA_ADDR + 8) as *mut u64;
        ptr::write_volatile(magic_before_ptr, 0xDEADBEEFCAFEBABE);
        ptr::write_volatile(magic_after_ptr, 0xABCDEF0123456789);
        
        // Write PML4 address
        let pml4_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::PML4) as *mut u64;
        ptr::write_volatile(pml4_ptr, pml4_addr);
        
        // IMMEDIATE VERIFICATION
        let verify_pml4_immediate = ptr::read_volatile(pml4_ptr);
        let verify_magic_before = ptr::read_volatile(magic_before_ptr);
        let verify_magic_after = ptr::read_volatile(magic_after_ptr);
        log::info!("  [VERIFY] Magic before @ {:#x}: {:#x}", magic_before_ptr as usize, verify_magic_before);
        log::info!("  [VERIFY] PML4 @ {:#x}: wrote {:#x}, read {:#x} {}", 
            pml4_ptr as usize, pml4_addr, verify_pml4_immediate,
            if verify_pml4_immediate == pml4_addr { "✓" } else { "✗ MISMATCH!" }
        );
        log::info!("  [VERIFY] Magic after @ {:#x}: {:#x}", magic_after_ptr as usize, verify_magic_after);
        
        // Write stack pointer
        let stack_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::STACK) as *mut u64;
        ptr::write_volatile(stack_ptr, stack_top);
        
        // Write CPU ID
        let cpu_id_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::CPU_ID) as *mut u64;
        ptr::write_volatile(cpu_id_ptr, cpu_id as u64);
        
        // Write entry point
        let entry_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::ENTRY) as *mut u64;
        ptr::write_volatile(entry_ptr, entry_point);
        
        // Write GDT descriptor (limit: u16, base: u64)
        let gdt_limit_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::GDT_DESC) as *mut u16;
        let gdt_base_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::GDT_DESC + 2) as *mut u64;
        ptr::write_volatile(gdt_limit_ptr, gdt_limit);
        ptr::write_volatile(gdt_base_ptr, gdt_base);
        
        // Write IDT descriptor (limit: u16, base: u64)
        let idt_limit_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::IDT_DESC) as *mut u16;
        let idt_base_ptr = (TRAMPOLINE_DATA_ADDR + data_offsets::IDT_DESC + 2) as *mut u64;
        ptr::write_volatile(idt_limit_ptr, idt_limit);
        ptr::write_volatile(idt_base_ptr, idt_base);
        
        // Verify IDT
        let verify_idt_limit = ptr::read_volatile(idt_limit_ptr);
        let verify_idt_base = ptr::read_volatile(idt_base_ptr);
        log::info!("  [VERIFY] IDT @ {:#x}: limit={:#x} (wrote {:#x}), base={:#x} (wrote {:#x})",
            idt_limit_ptr as usize, verify_idt_limit, idt_limit, verify_idt_base, idt_base);
        
        // ====================================================================
        // STEP 4: Verify data integrity
        // ====================================================================
        
        let verify_pml4 = ptr::read_volatile(pml4_ptr);
        let verify_stack = ptr::read_volatile(stack_ptr);
        
        if verify_pml4 != pml4_addr || verify_stack != stack_top {
            log::error!(
                "Data verification failed! PML4: wrote {:#x}, read {:#x}; Stack: wrote {:#x}, read {:#x}",
                pml4_addr, verify_pml4, stack_top, verify_stack
            );
            return Err("Data verification failed");
        }
        
        log::debug!("Boot data verified successfully");
        
        // ====================================================================
        // STEP 5: Calculate SIPI vector
        // ====================================================================
        
        // SIPI vector = physical_address / 4096
        let vector = (TRAMPOLINE_CODE_ADDR / 0x1000) as u8;
        
        log::info!("AP {} trampoline ready, SIPI vector = {:#x}", cpu_id, vector);
        
        Ok(vector)
    }
}

/// Read CR3 register (page table root)
#[inline]
fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

/// Allocate a kernel stack for an Application Processor
///
/// # Arguments
/// * `cpu_id` - Logical CPU ID
///
/// # Returns
/// Virtual address of the stack top (grows downward) or error
///
/// # Safety
/// The allocated stack is intentionally leaked to persist for the AP's lifetime
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
    
    log::debug!("Allocated AP {} stack: top={:#x}, size={}KB", cpu_id, stack_top, AP_STACK_SIZE / 1024);
    
    Ok(stack_top)
}
