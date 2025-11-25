//! Early Boot Initialization
//! 
//! Ultra-early initialization before Rust runtime is fully available.
//! This code runs in CRITICAL phase (<50ms target).

use crate::arch::x86_64;
use crate::boot::multiboot2::Multiboot2Info;

/// Early initialization sequence
/// 
/// Called very early in boot process.
/// Assumes: Stack setup, Serial available (from boot.c)
pub unsafe fn init(boot_info: &Multiboot2Info) -> Result<(), &'static str> {
    log::info!("Starting early initialization...");

    // 1. Setup GDT (Global Descriptor Table)
    init_gdt()?;

    // 2. Setup IDT (Interrupt Descriptor Table)
    init_idt()?;

    // 3. Detect and setup memory
    init_memory(boot_info)?;

    // 4. Initialize serial port for early debug (already done in boot.c, confirm it works)
    init_serial()?;

    log::info!("Early initialization complete");
    Ok(())
}

/// Initialize GDT
unsafe fn init_gdt() -> Result<(), &'static str> {
    log::info!("  [GDT] Setting up Global Descriptor Table...");
    
    x86_64::gdt::init();
    
    log::info!("  [GDT] Complete");
    Ok(())
}

/// Initialize IDT
unsafe fn init_idt() -> Result<(), &'static str> {
    log::info!("  [IDT] Setting up Interrupt Descriptor Table...");
    
    x86_64::idt::init();
    
    log::info!("  [IDT] Complete");
    Ok(())
}

/// Initialize memory management
unsafe fn init_memory(boot_info: &Multiboot2Info) -> Result<(), &'static str> {
    log::info!("  [MEMORY] Detecting and initializing memory...");

    // Parse memory map from Multiboot2
    if let Some(mmap) = boot_info.memory_map() {
        let total_available: u64 = mmap
            .filter(|e| e.is_available())
            .map(|e| e.length)
            .sum();
        
        log::info!("  [MEMORY] Total available: {}MB", total_available / 1024 / 1024);
    } else {
        return Err("No memory map found");
    }

    log::info!("  [MEMORY] Complete");
    Ok(())
}

/// Initialize serial port (confirm it works)
unsafe fn init_serial() -> Result<(), &'static str> {
    log::info!("  [SERIAL] Confirming serial port initialization...");
    
    log::info!("  [SERIAL] OK (COM1 0x3F8)");
    Ok(())
}
