//! Kernel debug console.
//!
//! Normal terminal input/output is owned by the Ring1 stack:
//! `ps2_driver -> input_server -> tty_server -> fb_server`. Ring0 keeps only
//! the QEMU debugcon writer and a handoff marker used by the IRQ registration
//! path.

use core::sync::atomic::{AtomicBool, Ordering};

static RING1_KEYBOARD_ACTIVE: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn debug_byte(byte: u8) {
    unsafe {
        core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
    }
}

pub fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        debug_byte(byte);
    }
}

pub fn handoff_keyboard_to_ring1() {
    RING1_KEYBOARD_ACTIVE.store(true, Ordering::Release);
}

pub fn ring1_keyboard_active() -> bool {
    RING1_KEYBOARD_ACTIVE.load(Ordering::Acquire)
}
