//! Boot Module
//! 
//! Manages kernel boot sequence with phased initialization.

pub mod multiboot2;
pub mod phases;
pub mod early_init;
pub mod late_init;
pub mod recovery;

pub use multiboot2::Multiboot2Info;
pub use phases::BootPhase;
pub use recovery::{RecoveryMode, RecoveryReason};

/// Simplified boot sequence without Result (to avoid unwinding issues)
pub unsafe fn boot_sequence_simple(multiboot_magic: u32, multiboot_addr: usize) {
    crate::logger::early_print("[BOOT] boot_sequence_simple() entered\n");
    
    // Log banner
    log::info!("═══════════════════════════════════════════════════════");
    log::info!("  EXO-OS KERNEL v0.4.1 - Rust Initialization");
    log::info!("═══════════════════════════════════════════════════════");
    log::info!("");
    
    // Validate magic
    log::info!("Multiboot2 magic: {:#x}", multiboot_magic);
    
    crate::logger::early_print("[BOOT] boot_sequence_simple() completed\n");
}

/// Main boot sequence
pub unsafe fn boot_sequence(multiboot_magic: u32, multiboot_addr: usize) -> Result<(), &'static str> {
    // Debug before logger init
    crate::logger::early_print("[RUST] boot_sequence() START\n");
    
    // Initialize logger FIRST so we can see all boot messages
    crate::logger::init();
    
    crate::logger::early_print("[RUST] After logger::init()\n");
    
    log::info!("═══════════════════════════════════════════════════════");
    log::info!("  EXO-OS KERNEL v0.4.1 - Rust Initialization");
    log::info!("═══════════════════════════════════════════════════════");
    
    crate::logger::early_print("[RUST] After log::info banner\n");
    
    // Validate Multiboot2 magic
    if !Multiboot2Info::validate_magic(multiboot_magic) {
        log::error!("Invalid Multiboot2 magic number: {:#x}", multiboot_magic);
        return Err("Invalid Multiboot2 magic number");
    }
    log::info!("✓ Multiboot2 magic validated");

    crate::logger::early_print("[RUST] After magic validation\n");

    // Parse boot info
    let boot_info = Multiboot2Info::from_ptr(multiboot_addr)
        .ok_or("Failed to parse Multiboot2 info")?;
    log::info!("✓ Multiboot2 info parsed");
    
    crate::logger::early_print("[RUST] After multiboot2 parse\n");
    
    multiboot2::print_memory_map(&boot_info);

    // CRITICAL PHASE
    early_init::init(&boot_info)?;
    phases::execute_critical()?;

    // NORMAL PHASE
    late_init::init()?;
    phases::execute_normal()?;

    // DEFERRED PHASE
    phases::execute_deferred()?;

    // Complete
    phases::complete()?;

    Ok(())
}
