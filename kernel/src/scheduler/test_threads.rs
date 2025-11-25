//! Test threads for scheduler demonstration
//! 
//! Three simple threads that display different messages on VGA

/// Test thread 1: Displays "Thread A" on line 18
pub fn thread_a() -> ! {
    let vga = 0xB8000 as *mut u16;
    let mut counter = 0u64;
    
    loop {
        unsafe {
            // Display thread name
            let msg = b"[Thread A] Count: ";
            for (i, &byte) in msg.iter().enumerate() {
                *vga.add(18 * 80 + i) = 0x0C00 | byte as u16; // Red
            }
            
            // Display counter
            display_number(vga, 18, 18, counter);
            
            counter = counter.wrapping_add(1);
            
            // Busy wait a bit to see the switching
            for _ in 0..100_000 {
                core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
            }
        }
    }
}

/// Test thread 2: Displays "Thread B" on line 19
pub fn thread_b() -> ! {
    let vga = 0xB8000 as *mut u16;
    let mut counter = 0u64;
    
    loop {
        unsafe {
            // Display thread name
            let msg = b"[Thread B] Count: ";
            for (i, &byte) in msg.iter().enumerate() {
                *vga.add(19 * 80 + i) = 0x0E00 | byte as u16; // Yellow
            }
            
            // Display counter
            display_number(vga, 19, 18, counter);
            
            counter = counter.wrapping_add(1);
            
            // Busy wait a bit to see the switching
            for _ in 0..100_000 {
                core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
            }
        }
    }
}

/// Test thread 3: Displays "Thread C" on line 20
pub fn thread_c() -> ! {
    let vga = 0xB8000 as *mut u16;
    let mut counter = 0u64;
    
    loop {
        unsafe {
            // Display thread name
            let msg = b"[Thread C] Count: ";
            for (i, &byte) in msg.iter().enumerate() {
                *vga.add(20 * 80 + i) = 0x0B00 | byte as u16; // Cyan
            }
            
            // Display counter
            display_number(vga, 20, 18, counter);
            
            counter = counter.wrapping_add(1);
            
            // Busy wait a bit to see the switching
            for _ in 0..100_000 {
                core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
            }
        }
    }
}

/// Helper to display a number on VGA
unsafe fn display_number(vga: *mut u16, row: usize, col: usize, mut num: u64) {
    const HEX_CHARS: &[u8] = b"0123456789ABCDEF";
    
    // Display as hex (16 digits)
    for i in 0..16 {
        let nibble = ((num >> ((15 - i) * 4)) & 0xF) as usize;
        *vga.add(row * 80 + col + i) = 0x0F00 | HEX_CHARS[nibble] as u16;
    }
}
