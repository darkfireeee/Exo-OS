//! Diagnostic des privilèges I/O et problèmes PIC
//! 
//! Module pour diagnostiquer les problèmes de privilèges I/O (IOPL)
//! et tester les opérations sur les ports.

use core::arch::asm;

/// Affiche l'état des privilèges I/O et effectue des tests (VERSION COMPACTE)
pub fn diagnose_io_privileges() {
    clear_diag_area();
    
    // Ligne compacte pour RFLAGS
    let rflags = read_rflags();
    let iopl = (rflags >> 12) & 0x3;
    
    vga_print_at(0, 0, b"[DIAG] IOPL=");
    print_decimal_at(0, 13, iopl as u32);
    
    if iopl < 3 {
        vga_print_at(0, 15, b"(WARN: need fix)");
    } else {
        vga_print_at(0, 15, b"(OK)            ");
    }
    
    // Tests I/O compacts
    vga_print_at(1, 0, b"[DIAG] I/O Tests: ");
    
    match safe_inb(0x80) {
        Ok(_) => vga_print_at(1, 18, b"0x80:OK "),
        Err(_) => vga_print_at(1, 18, b"0x80:ERR"),
    }
    
    match safe_inb(0x20) {
        Ok(_) => vga_print_at(1, 27, b"PIC:OK "),
        Err(_) => vga_print_at(1, 27, b"PIC:ERR"),
    }
    
    vga_print_at(1, 35, b"[Complete]");
}

/// Lit RFLAGS
#[inline]
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
#[inline]
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

/// Lit TR (Task Register)
#[inline]
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

/// Test d'I/O sécurisé (lecture)
fn safe_inb(port: u16) -> Result<u8, ()> {
    let value: u8;
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

/// Test d'I/O sécurisé (écriture)
fn safe_outb(port: u16, value: u8) -> Result<(), ()> {
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
    vga_print(b"[IO] Setting IOPL=3 in RFLAGS...\n");
    
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
    vga_print(b"[IO] New IOPL: ");
    print_decimal(new_iopl as u32);
    vga_print(b"\n");
}

// === Fonctions d'affichage VGA ===

/// Efface la zone de diagnostic (lignes 0-2)
fn clear_diag_area() {
    let vga_buffer = 0xB8000 as *mut u16;
    unsafe {
        for row in 0..3 {
            for col in 0..80 {
                let offset = row * 80 + col;
                vga_buffer.add(offset).write_volatile(0x0700);
            }
        }
    }
}

/// Affiche un message à une position précise
fn vga_print_at(row: usize, col: usize, msg: &[u8]) {
    let vga_buffer = 0xB8000 as *mut u16;
    unsafe {
        for (i, &byte) in msg.iter().enumerate() {
            if col + i < 80 {
                let offset = row * 80 + col + i;
                vga_buffer.add(offset).write_volatile((byte as u16) | 0x0E00);
            }
        }
    }
}

fn vga_print(msg: &[u8]) {
    static mut ROW: usize = 0;
    static mut COL: usize = 0;
    
    let vga_buffer = 0xB8000 as *mut u16;
    
    unsafe {
        for &byte in msg {
            if byte == b'\n' {
                ROW += 1;
                COL = 0;
                if ROW >= 25 {
                    ROW = 0;
                }
            } else {
                let offset = ROW * 80 + COL;
                vga_buffer.add(offset).write_volatile((byte as u16) | 0x0F00);
                COL += 1;
                if COL >= 80 {
                    COL = 0;
                    ROW += 1;
                    if ROW >= 25 {
                        ROW = 0;
                    }
                }
            }
        }
    }
}

fn print_hex_u64(value: u64) {
    vga_print(b"0x");
    for i in (0..16).rev() {
        let nibble = ((value >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'A' + (nibble - 10) };
        vga_print(&[c]);
    }
}

fn print_hex_u16(value: u16) {
    vga_print(b"0x");
    for i in (0..4).rev() {
        let nibble = ((value >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'A' + (nibble - 10) };
        vga_print(&[c]);
    }
}

fn print_hex_u8(value: u8) {
    vga_print(b"0x");
    for i in (0..2).rev() {
        let nibble = ((value >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'A' + (nibble - 10) };
        vga_print(&[c]);
    }
}

fn print_decimal(mut value: u32) {
    if value == 0 {
        vga_print(b"0");
        return;
    }
    
    let mut buffer = [0u8; 10];
    let mut i = 0;
    
    while value > 0 {
        buffer[i] = b'0' + (value % 10) as u8;
        value /= 10;
        i += 1;
    }
    
    for j in (0..i).rev() {
        vga_print(&[buffer[j]]);
    }
}

/// Affiche un nombre décimal à une position précise
fn print_decimal_at(row: usize, col: usize, value: u32) {
    let vga_buffer = 0xB8000 as *mut u16;
    let digit = b'0' + (value % 10) as u8;
    unsafe {
        let offset = row * 80 + col;
        vga_buffer.add(offset).write_volatile((digit as u16) | 0x0E00);
    }
}
