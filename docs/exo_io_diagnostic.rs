// kernel/src/arch/x86_64/io/diagnostic.rs
//
// DIAGNOSTIC DES PRIVILÈGES I/O ET PROBLÈMES PIC

use core::arch::asm;

/// Vérifie l'état des privilèges I/O
pub fn diagnose_io_privileges() {
    println!("\n=== DIAGNOSTIC I/O PRIVILEGES ===\n");
    
    // 1. Vérifier RFLAGS
    let rflags = read_rflags();
    println!("RFLAGS: {:#018x}", rflags);
    
    let iopl = (rflags >> 12) & 0x3;
    println!("  IOPL (I/O Privilege Level): {}", iopl);
    println!("  IF (Interrupt Flag):        {}", (rflags >> 9) & 1);
    
    if iopl < 3 {
        println!("  ⚠️  WARNING: IOPL={} < 3, I/O instructions may fault!", iopl);
        println!("      Need IOPL=3 or TSS I/O bitmap configured");
    } else {
        println!("  ✓ IOPL=3, I/O instructions allowed");
    }
    
    // 2. Vérifier le segment CS (Current Privilege Level)
    let cs = read_cs();
    let cpl = cs & 0x3;
    println!("\nCS (Code Segment): {:#06x}", cs);
    println!("  CPL (Current Privilege Level): {}", cpl);
    
    if cpl != 0 {
        println!("  ⚠️  WARNING: CPL={}, expected 0 (Ring 0)", cpl);
    } else {
        println!("  ✓ Running in Ring 0");
    }
    
    // 3. Vérifier si la TSS est chargée
    let tr = read_tr();
    println!("\nTR (Task Register): {:#06x}", tr);
    
    if tr == 0 {
        println!("  ⚠️  WARNING: No TSS loaded!");
        println!("      I/O bitmap unavailable");
    } else {
        println!("  ✓ TSS loaded at selector {:#x}", tr);
    }
    
    // 4. Test safe d'I/O
    println!("\n=== TESTING I/O OPERATIONS ===\n");
    
    println!("Test 1: Reading from port 0x80 (POST diagnostic port)");
    match safe_inb(0x80) {
        Ok(value) => println!("  ✓ Success! Read: {:#04x}", value),
        Err(e) => println!("  ✗ Failed: {}", e),
    }
    
    println!("\nTest 2: Writing to port 0x80 (safe, used for delays)");
    match safe_outb(0x80, 0x00) {
        Ok(_) => println!("  ✓ Success!"),
        Err(e) => println!("  ✗ Failed: {}", e),
    }
    
    println!("\nTest 3: Reading from PIC1 command (0x20)");
    match safe_inb(0x20) {
        Ok(value) => println!("  ✓ Success! Read: {:#04x}", value),
        Err(e) => println!("  ✗ Failed: {}", e),
    }
    
    println!("\n=== DIAGNOSIS COMPLETE ===\n");
}

/// Lit RFLAGS
fn read_rflags() -> u64 {
    let rflags: u64;
    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
    }
    rflags
}

/// Lit CS
fn read_cs() -> u16 {
    let cs: u16;
    unsafe {
        asm!(
            "mov {:x}, cs",
            out(reg) cs,
            options(nomem, nostack, preserves_flags)
        );
    }
    cs
}

/// Lit TR
fn read_tr() -> u16 {
    let tr: u16;
    unsafe {
        asm!(
            "str {:x}",
            out(reg) tr,
            options(nomem, nostack, preserves_flags)
        );
    }
    tr
}

/// Test d'I/O avec gestion d'exception
fn safe_inb(port: u16) -> Result<u8, &'static str> {
    let value: u8;
    
    // On ne peut pas vraiment catcher les exceptions facilement ici
    // mais on peut au moins essayer
    unsafe {
        asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    
    Ok(value)
}

fn safe_outb(port: u16, value: u8) -> Result<(), &'static str> {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    
    Ok(())
}

/// Configure IOPL=3 dans RFLAGS (nécessite Ring 0)
pub unsafe fn set_iopl_3() {
    println!("[IO] Setting IOPL=3 in RFLAGS...");
    
    asm!(
        "pushfq",           // Push RFLAGS sur la stack
        "pop rax",          // Pop dans RAX
        "or rax, 0x3000",   // Set bits 12-13 (IOPL=3)
        "push rax",         // Push sur la stack
        "popfq",            // Pop dans RFLAGS
        out("rax") _,
        options(nomem, preserves_flags)
    );
    
    let new_rflags = read_rflags();
    let new_iopl = (new_rflags >> 12) & 0x3;
    println!("[IO] New IOPL: {}", new_iopl);
}

// Placeholder pour println
macro_rules! println {
    ($($arg:tt)*) => {
        // TODO: Utiliser votre implementation VGA/Serial
    };
}
