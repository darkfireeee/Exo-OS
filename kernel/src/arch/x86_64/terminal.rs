//! Minimal production console used by Ring3 stdio.
//!
//! This module deliberately stays small: stdout/stderr render to the visible
//! boot display, while stdin polls the PS/2 controller and returns decoded ASCII
//! bytes. The full input_server/tty_server stack still starts in userspace; this
//! path guarantees PID1 and the first shell have a usable controlling console.

use crate::scheduler::sync::wait_queue::WaitQueue;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use super::{boot_display, inb};

const KEYBOARD_MODE_UNKNOWN: u8 = 0;
const KEYBOARD_MODE_SET1: u8 = 1;
const KEYBOARD_MODE_SET2: u8 = 2;
const I8042_STATUS_OUTPUT_FULL: u8 = 1 << 0;
const KEYBOARD_BUFFER_LEN: usize = 128;
const KEYBOARD_DRAIN_LIMIT: usize = 64;

#[derive(Clone, Copy)]
struct KeyboardState {
    shift: bool,
    ctrl: bool,
    alt: bool,
    extended: bool,
    set2_release: bool,
    buf: [u8; KEYBOARD_BUFFER_LEN],
    head: usize,
    tail: usize,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            shift: false,
            ctrl: false,
            alt: false,
            extended: false,
            set2_release: false,
            buf: [0; KEYBOARD_BUFFER_LEN],
            head: 0,
            tail: 0,
        }
    }

    fn pop_byte(&mut self) -> Option<u8> {
        if self.head == self.tail {
            return None;
        }
        let byte = self.buf[self.tail];
        self.tail = (self.tail + 1) % KEYBOARD_BUFFER_LEN;
        Some(byte)
    }

    fn push_byte(&mut self, byte: u8) {
        let next = (self.head + 1) % KEYBOARD_BUFFER_LEN;
        if next == self.tail {
            self.tail = (self.tail + 1) % KEYBOARD_BUFFER_LEN;
        }
        self.buf[self.head] = byte;
        self.head = next;
    }
}

static KEYBOARD: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());
static KEYBOARD_WAIT: WaitQueue = WaitQueue::new();
static FOREGROUND_PGID: AtomicU32 = AtomicU32::new(0);
static KEYBOARD_MODE: AtomicU32 = AtomicU32::new(KEYBOARD_MODE_UNKNOWN as u32);

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

fn normalize_pgid(pid: u32, pgid: u32) -> u32 {
    if pgid == 0 {
        pid
    } else {
        pgid
    }
}

fn foreground_owner(pid: u32, pgid: u32) -> u32 {
    normalize_pgid(pid, pgid)
}

fn claim_foreground(pid: u32, pgid: u32) -> u32 {
    let owner = foreground_owner(pid, pgid);
    if owner != 0 {
        FOREGROUND_PGID.store(owner, Ordering::Release);
    }
    owner
}

fn claim_foreground_if_unclaimed(pid: u32, pgid: u32) -> u32 {
    let owner = normalize_pgid(pid, pgid);
    if owner != 0 {
        let _ = FOREGROUND_PGID.compare_exchange(0, owner, Ordering::AcqRel, Ordering::Acquire);
    }
    owner
}

fn is_foreground(pid: u32, pgid: u32) -> bool {
    let owner = FOREGROUND_PGID.load(Ordering::Acquire);
    owner != 0 && owner == normalize_pgid(pid, pgid)
}

fn requests_terminal_claim(bytes: &[u8]) -> bool {
    bytes.first().copied() == Some(0x0c)
        || bytes
            .windows(b"Exo-OS userspace console ready".len())
            .any(|window| window == b"Exo-OS userspace console ready")
}

/// Raw visible-console write for kernel-owned foreground messages.
pub fn write(bytes: &[u8]) {
    debug_write(bytes);
    boot_display::terminal_write_bytes(bytes);
}

/// Writes process stdio to the terminal discipline.
///
/// All bytes are mirrored to the QEMU debugcon. The visible framebuffer is
/// reserved for the foreground console process group, claimed by the first
/// interactive shell clear/banner or by the first stdin reader.
pub fn write_from_process(pid: u32, pgid: u32, bytes: &[u8]) {
    debug_write(bytes);
    if requests_terminal_claim(bytes) {
        claim_foreground(pid, pgid);
    }
    if is_foreground(pid, pgid) {
        boot_display::terminal_write_bytes(bytes);
    }
}

pub fn clear() {
    debug_byte(0x0c);
    boot_display::terminal_clear();
}

pub fn poll_byte_for_process(pid: u32, pgid: u32) -> Option<u8> {
    claim_foreground_if_unclaimed(pid, pgid);
    if !is_foreground(pid, pgid) {
        return None;
    }
    poll_byte()
}

pub fn poll_byte() -> Option<u8> {
    let mode = keyboard_mode();
    let flags = super::irq_save();
    let result = {
        let mut state = KEYBOARD.lock();
        poll_byte_locked(&mut state, mode)
    };
    super::irq_restore(flags);
    result
}

/// Blocking stdin byte read for the foreground console process.
pub fn read_byte_for_process(pid: u32, pgid: u32) -> Result<Option<u8>, ()> {
    claim_foreground_if_unclaimed(pid, pgid);
    if !is_foreground(pid, pgid) {
        return Ok(None);
    }

    loop {
        if let Some(byte) = poll_byte() {
            return Ok(Some(byte));
        }

        let tcb = crate::scheduler::core::switch::current_thread_raw();
        if tcb.is_null() {
            return Ok(None);
        }

        // SAFETY: `tcb` is the current thread; WaitQueue owns the transient node.
        if !unsafe { KEYBOARD_WAIT.wait_interruptible(tcb) } {
            return Err(());
        }
    }
}

/// IRQ1 path: drain PS/2 scancodes into the shared keyboard buffer and wake readers.
pub fn keyboard_irq_drain() {
    let mode = match KEYBOARD_MODE.load(Ordering::Acquire) as u8 {
        KEYBOARD_MODE_UNKNOWN => KEYBOARD_MODE_SET1,
        mode => mode,
    };
    let mut produced = false;
    {
        let mut state = KEYBOARD.lock();
        let mut drained = 0usize;
        while drained < KEYBOARD_DRAIN_LIMIT {
            let status = unsafe { inb(0x64) };
            if status & I8042_STATUS_OUTPUT_FULL == 0 {
                break;
            }
            let scancode = unsafe { inb(0x60) };
            if let Some(byte) = decode_scancode(&mut state, scancode, mode) {
                state.push_byte(byte);
                produced = true;
            }
            drained += 1;
        }
    }
    if produced {
        KEYBOARD_WAIT.notify_all();
    }
}

fn poll_byte_locked(state: &mut KeyboardState, mode: u8) -> Option<u8> {
    let mut first = state.pop_byte();
    let mut drained = 0usize;
    while drained < KEYBOARD_DRAIN_LIMIT {
        let status = unsafe { inb(0x64) };
        if status & I8042_STATUS_OUTPUT_FULL == 0 {
            break;
        }

        let scancode = unsafe { inb(0x60) };
        if let Some(byte) = decode_scancode(state, scancode, mode) {
            if first.is_none() {
                first = Some(byte);
            } else {
                state.push_byte(byte);
            }
        }
        drained += 1;
    }

    first
}

fn keyboard_mode() -> u8 {
    let current = KEYBOARD_MODE.load(Ordering::Acquire) as u8;
    if current != KEYBOARD_MODE_UNKNOWN {
        return current;
    }
    let detected = detect_keyboard_mode();
    let _ = KEYBOARD_MODE.compare_exchange(
        KEYBOARD_MODE_UNKNOWN as u32,
        detected as u32,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    KEYBOARD_MODE.load(Ordering::Acquire) as u8
}

fn detect_keyboard_mode() -> u8 {
    // QEMU HMP/GUI keyboard injection feeds translated set-1 bytes even when
    // the controller config read reports translation disabled. Starting from
    // set-2 in that case turns a set-1 Enter make code (0x1C) into "a".
    //
    // Keep the boot path conservative: decode set-1 until the live stream
    // proves it is set-2 by emitting the 0xF0 break prefix. `decode_scancode`
    // already records that prefix and promotes KEYBOARD_MODE to set-2.
    KEYBOARD_MODE_SET1
}

fn decode_scancode(state: &mut KeyboardState, scancode: u8, mode: u8) -> Option<u8> {
    match scancode {
        0xE0 => {
            state.extended = true;
            return None;
        }
        0xF0 => {
            state.set2_release = true;
            KEYBOARD_MODE.store(KEYBOARD_MODE_SET2 as u32, Ordering::Release);
            return None;
        }
        _ => {}
    }

    let extended = state.extended;
    state.extended = false;
    if mode == KEYBOARD_MODE_SET2 {
        let released = state.set2_release;
        state.set2_release = false;
        if update_modifier_set2(state, scancode, released) {
            return None;
        }
        if extended && !released {
            return set2_extended_sequence(state, scancode);
        }
        if extended || released {
            return None;
        }
        return set2_ascii(scancode, state.shift, state.ctrl);
    }

    let is_set1_release = scancode & 0x80 != 0;
    let raw = scancode & 0x7F;
    let released = is_set1_release || state.set2_release;
    state.set2_release = false;

    if update_modifier_set1(state, raw, released) {
        return None;
    }
    if extended {
        return if released {
            None
        } else {
            set1_extended_sequence(state, raw)
        };
    }
    if released {
        return None;
    }

    set1_ascii(raw, state.shift, state.ctrl)
}

fn emit_sequence(state: &mut KeyboardState, seq: &[u8]) -> Option<u8> {
    if seq.is_empty() {
        return None;
    }
    let mut i = 1usize;
    while i < seq.len() {
        state.push_byte(seq[i]);
        i += 1;
    }
    Some(seq[0])
}

fn set1_extended_sequence(state: &mut KeyboardState, raw: u8) -> Option<u8> {
    match raw {
        0x48 => emit_sequence(state, b"\x1b[A"),
        0x50 => emit_sequence(state, b"\x1b[B"),
        0x4D => emit_sequence(state, b"\x1b[C"),
        0x4B => emit_sequence(state, b"\x1b[D"),
        _ => None,
    }
}

fn set2_extended_sequence(state: &mut KeyboardState, raw: u8) -> Option<u8> {
    match raw {
        0x75 => emit_sequence(state, b"\x1b[A"),
        0x72 => emit_sequence(state, b"\x1b[B"),
        0x74 => emit_sequence(state, b"\x1b[C"),
        0x6B => emit_sequence(state, b"\x1b[D"),
        _ => None,
    }
}

fn update_modifier_set1(state: &mut KeyboardState, raw: u8, released: bool) -> bool {
    let pressed = !released;
    match raw {
        0x2A | 0x36 => {
            state.shift = pressed;
            true
        }
        0x1D => {
            state.ctrl = pressed;
            true
        }
        0x38 => {
            state.alt = pressed;
            true
        }
        _ => false,
    }
}

fn update_modifier_set2(state: &mut KeyboardState, raw: u8, released: bool) -> bool {
    let pressed = !released;
    match raw {
        0x12 | 0x59 => {
            state.shift = pressed;
            true
        }
        0x14 => {
            state.ctrl = pressed;
            true
        }
        0x11 => {
            state.alt = pressed;
            true
        }
        _ => false,
    }
}

fn set1_ascii(raw: u8, shift: bool, ctrl: bool) -> Option<u8> {
    let ch = match raw {
        0x01 => 0x1B,
        0x02 => shifted(b'1', b'!', shift),
        0x03 => shifted(b'2', b'@', shift),
        0x04 => shifted(b'3', b'#', shift),
        0x05 => shifted(b'4', b'$', shift),
        0x06 => shifted(b'5', b'%', shift),
        0x07 => shifted(b'6', b'^', shift),
        0x08 => shifted(b'7', b'&', shift),
        0x09 => shifted(b'8', b'*', shift),
        0x0A => shifted(b'9', b'(', shift),
        0x0B => shifted(b'0', b')', shift),
        0x0C => shifted(b'-', b'_', shift),
        0x0D => shifted(b'=', b'+', shift),
        0x0E => 0x08,
        0x0F => b'\t',
        0x10 => letter(b'q', shift),
        0x11 => letter(b'w', shift),
        0x12 => letter(b'e', shift),
        0x13 => letter(b'r', shift),
        0x14 => letter(b't', shift),
        0x15 => letter(b'y', shift),
        0x16 => letter(b'u', shift),
        0x17 => letter(b'i', shift),
        0x18 => letter(b'o', shift),
        0x19 => letter(b'p', shift),
        0x1A => shifted(b'[', b'{', shift),
        0x1B => shifted(b']', b'}', shift),
        0x1C => b'\n',
        0x1E => letter(b'a', shift),
        0x1F => letter(b's', shift),
        0x20 => letter(b'd', shift),
        0x21 => letter(b'f', shift),
        0x22 => letter(b'g', shift),
        0x23 => letter(b'h', shift),
        0x24 => letter(b'j', shift),
        0x25 => letter(b'k', shift),
        0x26 => letter(b'l', shift),
        0x27 => shifted(b';', b':', shift),
        0x28 => shifted(b'\'', b'"', shift),
        0x29 => shifted(b'`', b'~', shift),
        0x2B => shifted(b'\\', b'|', shift),
        0x2C => letter(b'z', shift),
        0x2D => letter(b'x', shift),
        0x2E => letter(b'c', shift),
        0x2F => letter(b'v', shift),
        0x30 => letter(b'b', shift),
        0x31 => letter(b'n', shift),
        0x32 => letter(b'm', shift),
        0x33 => shifted(b',', b'<', shift),
        0x34 => shifted(b'.', b'>', shift),
        0x35 => shifted(b'/', b'?', shift),
        0x39 => b' ',
        _ => return None,
    };

    if ctrl && ch.is_ascii_alphabetic() {
        Some((ch.to_ascii_lowercase() - b'a') + 1)
    } else {
        Some(ch)
    }
}

fn set2_ascii(raw: u8, shift: bool, ctrl: bool) -> Option<u8> {
    let ch = match raw {
        0x76 => 0x1B,
        0x16 => shifted(b'1', b'!', shift),
        0x1E => shifted(b'2', b'@', shift),
        0x26 => shifted(b'3', b'#', shift),
        0x25 => shifted(b'4', b'$', shift),
        0x2E => shifted(b'5', b'%', shift),
        0x36 => shifted(b'6', b'^', shift),
        0x3D => shifted(b'7', b'&', shift),
        0x3E => shifted(b'8', b'*', shift),
        0x46 => shifted(b'9', b'(', shift),
        0x45 => shifted(b'0', b')', shift),
        0x4E => shifted(b'-', b'_', shift),
        0x55 => shifted(b'=', b'+', shift),
        0x66 => 0x08,
        0x0D => b'\t',
        0x15 => letter(b'q', shift),
        0x1D => letter(b'w', shift),
        0x24 => letter(b'e', shift),
        0x2D => letter(b'r', shift),
        0x2C => letter(b't', shift),
        0x35 => letter(b'y', shift),
        0x3C => letter(b'u', shift),
        0x43 => letter(b'i', shift),
        0x44 => letter(b'o', shift),
        0x4D => letter(b'p', shift),
        0x54 => shifted(b'[', b'{', shift),
        0x5B => shifted(b']', b'}', shift),
        0x5A => b'\n',
        0x1C => letter(b'a', shift),
        0x1B => letter(b's', shift),
        0x23 => letter(b'd', shift),
        0x2B => letter(b'f', shift),
        0x34 => letter(b'g', shift),
        0x33 => letter(b'h', shift),
        0x3B => letter(b'j', shift),
        0x42 => letter(b'k', shift),
        0x4B => letter(b'l', shift),
        0x4C => shifted(b';', b':', shift),
        0x52 => shifted(b'\'', b'"', shift),
        0x0E => shifted(b'`', b'~', shift),
        0x5D => shifted(b'\\', b'|', shift),
        0x1A => letter(b'z', shift),
        0x22 => letter(b'x', shift),
        0x21 => letter(b'c', shift),
        0x2A => letter(b'v', shift),
        0x32 => letter(b'b', shift),
        0x31 => letter(b'n', shift),
        0x3A => letter(b'm', shift),
        0x41 => shifted(b',', b'<', shift),
        0x49 => shifted(b'.', b'>', shift),
        0x4A => shifted(b'/', b'?', shift),
        0x29 => b' ',
        _ => return None,
    };

    if ctrl && ch.is_ascii_alphabetic() {
        Some((ch.to_ascii_lowercase() - b'a') + 1)
    } else {
        Some(ch)
    }
}

#[inline]
const fn shifted(base: u8, shifted: u8, shift: bool) -> u8 {
    if shift {
        shifted
    } else {
        base
    }
}

#[inline]
const fn letter(lower: u8, shift: bool) -> u8 {
    if shift {
        lower - 32
    } else {
        lower
    }
}
