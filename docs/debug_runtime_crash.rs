// kernel/src/main_debug.rs
//
// VERSION DEBUG DE RUST_MAIN POUR ISOLER LE CRASH

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// Fonction C externe pour serial output
extern "C" {
    fn serial_puts(s: *const u8);
    fn serial_putc(c: u8);
}

/// Helper pour afficher un string
unsafe fn debug_print(msg: &str) {
    // Version ULTRA-SAFE: un char à la fois
    for byte in msg.as_bytes() {
        serial_putc(*byte);
    }
    serial_putc(b'\n');
}

/// Helper pour afficher un nombre en hexa
unsafe fn debug_print_hex(name: &str, value: u64) {
    debug_print(name);
    
    // Afficher en hexa manuellement
    let hex_chars = b"0123456789ABCDEF";
    serial_putc(b'0');
    serial_putc(b'x');
    
    for i in (0..16).rev() {
        let nibble = ((value >> (i * 4)) & 0xF) as usize;
        serial_putc(hex_chars[nibble]);
    }
    serial_putc(b'\n');
}

/// Point d'entrée principal (appelé depuis C)
#[no_mangle]
pub extern "C" fn rust_main(multiboot_magic: u32, multiboot_info: usize) -> ! {
    unsafe {
        // TEST 1: Affichage simple
        debug_print("=== RUST DEBUG START ===");
        
        // TEST 2: Afficher les paramètres
        debug_print("Multiboot Magic:");
        debug_print_hex("", multiboot_magic as u64);
        
        debug_print("Multiboot Info:");
        debug_print_hex("", multiboot_info as u64);
        
        // TEST 3: Vérifier la stack
        let stack_ptr: u64;
        core::arch::asm!("mov {}, rsp", out(reg) stack_ptr);
        debug_print("Stack Pointer:");
        debug_print_hex("", stack_ptr);
        
        // TEST 4: Allouer sur la stack (petit)
        let small_array = [0u8; 16];
        debug_print("Small stack alloc OK");
        
        // TEST 5: Allouer plus (attention!)
        let medium_array = [0u8; 256];
        debug_print("Medium stack alloc OK");
        
        // TEST 6: Écrire dans les tableaux
        core::ptr::write_volatile(&small_array[0] as *const u8 as *mut u8, 0x42);
        debug_print("Write to small array OK");
        
        core::ptr::write_volatile(&medium_array[0] as *const u8 as *mut u8, 0x69);
        debug_print("Write to medium array OK");
        
        // TEST 7: Lire les valeurs
        let val1 = core::ptr::read_volatile(&small_array[0]);
        let val2 = core::ptr::read_volatile(&medium_array[0]);
        
        debug_print("Read values:");
        debug_print_hex("  small[0]", val1 as u64);
        debug_print_hex("  medium[0]", val2 as u64);
        
        // TEST 8: Allocation large (risqué!)
        debug_print("Attempting large stack alloc...");
        let large_array = [0u8; 4096];  // 4KB
        debug_print("Large stack alloc OK");
        
        // TEST 9: Vérifier après alloc
        let stack_ptr_after: u64;
        core::arch::asm!("mov {}, rsp", out(reg) stack_ptr_after);
        debug_print("Stack after alloc:");
        debug_print_hex("", stack_ptr_after);
        
        let stack_used = stack_ptr - stack_ptr_after;
        debug_print("Stack used:");
        debug_print_hex("", stack_used);
        
        // TEST 10: Loop infinie
        debug_print("=== ALL TESTS PASSED ===");
        debug_print("Entering infinite loop...");
        
        loop {
            core::arch::asm!("hlt");
        }
    }
}

/// Panic handler ultra-simple
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        debug_print("!!! PANIC !!!");
        
        if let Some(location) = info.location() {
            debug_print("Location:");
            // On ne peut pas formatter facilement, juste afficher "PANIC"
        }
        
        loop {
            core::arch::asm!("cli; hlt");
        }
    }
}
