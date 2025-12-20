//! PS/2 Keyboard Driver for x86_64
//! 
//! Implements basic PS/2 keyboard support with IRQ1 handling
//! Converts scan codes to ASCII using US keyboard layout

use core::arch::asm;
use spin::Mutex;
use alloc::collections::VecDeque;

/// PS/2 keyboard data port
const KEYBOARD_DATA_PORT: u16 = 0x60;

/// PS/2 keyboard status port
const KEYBOARD_STATUS_PORT: u16 = 0x64;

/// PS/2 keyboard command port
const KEYBOARD_COMMAND_PORT: u16 = 0x64;

/// Circular buffer for keyboard input
static KEYBOARD_BUFFER: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

/// Shift key state
static mut SHIFT_PRESSED: bool = false;

/// Scan code to ASCII conversion table (US layout, without shift)
const SCANCODE_TO_ASCII: [u8; 128] = [
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=', 8, // 0-14
    b'\t', b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', // 15-28
    0, b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`', // 29-42
    0, b'\\', b'z', b'x', b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/', 0, b'*', // 43-55
    0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 56-71
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 72-87
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 88-103
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 104-119
    0, 0, 0, 0, 0, 0, 0, 0, // 120-127
];

/// Scan code to ASCII conversion table (US layout, with shift)
const SCANCODE_TO_ASCII_SHIFT: [u8; 128] = [
    0, 27, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')', b'_', b'+', 8, // 0-14
    b'\t', b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', // 15-28
    0, b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', b'"', b'~', // 29-42
    0, b'|', b'Z', b'X', b'C', b'V', b'B', b'N', b'M', b'<', b'>', b'?', 0, b'*', // 43-55
    0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 56-71
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 72-87
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 88-103
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 104-119
    0, 0, 0, 0, 0, 0, 0, 0, // 120-127
];

/// Scan codes for special keys
const SCANCODE_LEFT_SHIFT: u8 = 0x2A;
const SCANCODE_RIGHT_SHIFT: u8 = 0x36;
const SCANCODE_CAPS_LOCK: u8 = 0x3A;

/// Initialize PS/2 keyboard
pub fn init() {
    log::info!("[PS2_KBD] Initializing PS/2 keyboard driver...");
    
    // Send self-test command
    unsafe {
        asm!("out 0x64, al", in("al") 0xAAu8, options(nomem, nostack));
    }
    
    // Wait for response
    for _ in 0..1000 {
        unsafe { core::arch::asm!("pause") };
    }
    
    // Read response (should be 0x55 for success)
    let response: u8;
    unsafe {
        asm!("in al, 0x60", out("al") response, options(nomem, nostack));
    }
    if response == 0x55 {
        log::info!("[PS2_KBD] Self-test passed (0x55)");
    } else {
        log::warn!("[PS2_KBD] Self-test response: 0x{:02X}", response);
    }
    
    // Enable keyboard
    unsafe {
        asm!("out 0x64, al", in("al") 0x60u8, options(nomem, nostack)); // Write command byte
        asm!("out 0x60, al", in("al") 0x47u8, options(nomem, nostack)); // Enable IRQ1, enable translation
    }
    
    log::info!("[PS2_KBD] ✅ PS/2 keyboard initialized");
}

/// Handle keyboard interrupt (IRQ1)
pub fn handle_irq() {
    // Read scan code from keyboard
    let scancode: u8;
    unsafe {
        asm!("in al, 0x60", out("al") scancode, options(nomem, nostack));
    }
    
    // Handle key release (scan code with bit 7 set)
    if scancode & 0x80 != 0 {
        let key = scancode & 0x7F;
        
        // Handle shift release
        if key == SCANCODE_LEFT_SHIFT || key == SCANCODE_RIGHT_SHIFT {
            unsafe { SHIFT_PRESSED = false; }
        }
        
        return; // Ignore key releases for now
    }
    
    // Handle key press
    match scancode {
        SCANCODE_LEFT_SHIFT | SCANCODE_RIGHT_SHIFT => {
            unsafe { SHIFT_PRESSED = true; }
        }
        SCANCODE_CAPS_LOCK => {
            // TODO: Toggle caps lock state
        }
        _ => {
            // Convert scan code to ASCII
            let ascii = unsafe {
                if SHIFT_PRESSED {
                    SCANCODE_TO_ASCII_SHIFT[scancode as usize]
                } else {
                    SCANCODE_TO_ASCII[scancode as usize]
                }
            };
            
            if ascii != 0 {
                // Add to buffer
                let mut buffer = KEYBOARD_BUFFER.lock();
                if buffer.len() < 256 {
                    buffer.push_back(ascii);
                    
                    // Echo character to serial (for debugging)
                    if ascii == b'\n' {
                        log::debug!("[PS2_KBD] Key pressed: <ENTER>");
                    } else if ascii >= 32 && ascii <= 126 {
                        log::debug!("[PS2_KBD] Key pressed: '{}'", ascii as char);
                    } else {
                        log::debug!("[PS2_KBD] Key pressed: 0x{:02X}", ascii);
                    }
                } else {
                    log::warn!("[PS2_KBD] Buffer full, dropping character");
                }
            }
        }
    }
}

/// Read a character from keyboard buffer (non-blocking)
pub fn read_char() -> Option<u8> {
    let mut buffer = KEYBOARD_BUFFER.lock();
    buffer.pop_front()
}

/// Read multiple characters from keyboard buffer
pub fn read_bytes(buf: &mut [u8]) -> usize {
    let mut buffer = KEYBOARD_BUFFER.lock();
    let mut count = 0;
    
    for i in 0..buf.len() {
        if let Some(ch) = buffer.pop_front() {
            buf[i] = ch;
            count += 1;
        } else {
            break;
        }
    }
    
    count
}

/// Check if keyboard buffer has data
pub fn has_data() -> bool {
    let buffer = KEYBOARD_BUFFER.lock();
    !buffer.is_empty()
}

/// Get number of characters in buffer
pub fn buffer_size() -> usize {
    let buffer = KEYBOARD_BUFFER.lock();
    buffer.len()
}

/// Clear keyboard buffer
pub fn clear_buffer() {
    let mut buffer = KEYBOARD_BUFFER.lock();
    buffer.clear();
}
