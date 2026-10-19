//! # exo-usb-hid - Decodeur HID Boot Protocol USB (Clavier et Souris) pour Exo-OS
//!
//! Specification USB Device Class Definition for Human Interface Devices (HID) v1.11.
//! Decode les trames d'entree brutes et genere des structures InputEventWire
//! transmises a input_server via INPUT_MSG_PUSH.

#![no_std]

// Pas de dependance superflue
use exo_syscall_abi as syscall;

// -- Modificateurs USB HID ----------------------------------------------------
pub const HID_MOD_LCTRL: u8  = 1 << 0;
pub const HID_MOD_LSHIFT: u8 = 1 << 1;
pub const HID_MOD_LALT: u8   = 1 << 2;
pub const HID_MOD_LMETA: u8  = 1 << 3;
pub const HID_MOD_RCTRL: u8  = 1 << 4;
pub const HID_MOD_RSHIFT: u8 = 1 << 5;
pub const HID_MOD_RALT: u8   = 1 << 6;
pub const HID_MOD_RMETA: u8  = 1 << 7;

/// Decodeur pour clavier USB HID Boot Protocol (rapport de 8 octets)
pub struct UsbHidKeyboard {
    prev_report: [u8; 8],
}

impl UsbHidKeyboard {
    pub const fn new() -> Self {
        Self {
            prev_report: [0u8; 8],
        }
    }

    /// Analyse un rapport HID de 8 octets :
    /// - byte 0 : modificateurs (Ctrl, Shift, Alt, Meta)
    /// - byte 1 : reserve (0)
    /// - bytes 2..8 : jusqu'a 6 touches actives simultanees (keycodes USB)
    ///
    /// Invoque callback pour chaque touche pressee ou relachee.
    pub fn process_report<F>(&mut self, report: &[u8; 8], mut callback: F)
    where
        F: FnMut(syscall::InputEventWire),
    {
        let modifiers = report[0];
        let shift = (modifiers & (HID_MOD_LSHIFT | HID_MOD_RSHIFT)) != 0;
        let ctrl = (modifiers & (HID_MOD_LCTRL | HID_MOD_RCTRL)) != 0;
        let alt = (modifiers & (HID_MOD_LALT | HID_MOD_RALT)) != 0;
        let meta = (modifiers & (HID_MOD_LMETA | HID_MOD_RMETA)) != 0;

        let mut mod_bits = 0u8;
        if shift { mod_bits |= syscall::INPUT_MOD_SHIFT; }
        if ctrl  { mod_bits |= syscall::INPUT_MOD_CTRL;  }
        if alt   { mod_bits |= syscall::INPUT_MOD_ALT;   }
        if meta  { mod_bits |= syscall::INPUT_MOD_META;  }

        // 1. Detecter les touches relachees (presentes dans prev, absentes du courant)
        for &prev_key in &self.prev_report[2..8] {
            if prev_key != 0 && !report[2..8].contains(&prev_key) {
                let ascii = hid_to_ascii(prev_key, shift);
                callback(syscall::InputEventWire {
                    device: syscall::INPUT_DEVICE_KEYBOARD,
                    state: syscall::INPUT_KEY_RELEASED,
                    code: prev_key as u16,
                    value: 0,
                    ascii,
                    modifiers: mod_bits,
                    _pad: [0; 4],
                });
            }
        }

        // 2. Detecter les nouvelles touches pressees (presentes dans courant, absentes de prev)
        for &curr_key in &report[2..8] {
            if curr_key != 0 && !self.prev_report[2..8].contains(&curr_key) {
                let ascii = hid_to_ascii(curr_key, shift);
                callback(syscall::InputEventWire {
                    device: syscall::INPUT_DEVICE_KEYBOARD,
                    state: syscall::INPUT_KEY_PRESSED,
                    code: curr_key as u16,
                    value: 1,
                    ascii,
                    modifiers: mod_bits,
                    _pad: [0; 4],
                });
            }
        }

        self.prev_report = *report;
    }
}

/// Decodeur pour souris USB HID Boot Protocol (rapport de 3 ou 4 octets)
pub struct UsbHidMouse {
    prev_buttons: u8,
}

impl UsbHidMouse {
    pub const fn new() -> Self {
        Self { prev_buttons: 0 }
    }

    /// Analyse un rapport souris :
    /// - byte 0 : boutons (bit 0 = gauche, bit 1 = droite, bit 2 = milieu)
    /// - byte 1 : deplacement X signe (i8)
    /// - byte 2 : deplacement Y signe (i8)
    /// - byte 3 (optionnel) : molette defilement (i8)
    pub fn process_report<F>(&mut self, report: &[u8], mut callback: F)
    where
        F: FnMut(syscall::InputEventWire),
    {
        if report.len() < 3 {
            return;
        }

        let buttons = report[0];
        let dx = report[1] as i8;
        let dy = report[2] as i8;

        // Bouton gauche
        let left_pressed = (buttons & 1) != 0;
        let prev_left = (self.prev_buttons & 1) != 0;
        if left_pressed != prev_left {
            callback(syscall::InputEventWire {
                device: syscall::INPUT_DEVICE_MOUSE,
                state: if left_pressed { syscall::INPUT_KEY_PRESSED } else { syscall::INPUT_KEY_RELEASED },
                code: 1, // Bouton 1 (Gauche)
                value: if left_pressed { 1 } else { 0 },
                ascii: 0,
                modifiers: 0,
                _pad: [0; 4],
            });
        }

        // Deplacement souris
        if dx != 0 || dy != 0 {
            callback(syscall::InputEventWire {
                device: syscall::INPUT_DEVICE_MOUSE,
                state: syscall::INPUT_KEY_PRESSED,
                code: 0, // Mouvement
                value: dx as i16,
                ascii: dy as u8,
                modifiers: 0,
                _pad: [0; 4],
            });
        }

        self.prev_buttons = buttons;
    }
}

/// Envoie une structure InputEventWire a input_server via INPUT_MSG_PUSH.
pub fn push_input_event(event: syscall::InputEventWire) {
    let req = syscall::InputRequest {
        sender_pid: 0,
        msg_type: syscall::INPUT_MSG_PUSH,
        reply_endpoint: 0,
        event,
    };
    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            0,
            0,
            0,
        );
    }
}

/// Conversion d'un usage code USB HID vers son caractere ASCII standard.
pub fn hid_to_ascii(usage: u8, shift: bool) -> u8 {
    match usage {
        // Lettres a-z / A-Z
        0x04..=0x1D => {
            let base = if shift { b'A' } else { b'a' };
            base + (usage - 0x04)
        }
        // Chiffres 1-9
        0x1E..=0x26 => {
            if shift {
                match usage {
                    0x1E => b'!',
                    0x1F => b'@',
                    0x20 => b'#',
                    0x21 => b'$',
                    0x22 => b'%',
                    0x23 => b'^',
                    0x24 => b'&',
                    0x25 => b'*',
                    0x26 => b'(',
                    _ => 0,
                }
            } else {
                b'1' + (usage - 0x1E)
            }
        }
        // Chiffre 0
        0x27 => if shift { b')' } else { b'0' },
        0x28 => b'\n',   // Entree
        0x29 => 27,      // Echap
        0x2A => 0x08,    // Retour arriere
        0x2B => b'\t',   // Tabulation
        0x2C => b' ',    // Espace
        0x2D => if shift { b'_' } else { b'-' },
        0x2E => if shift { b'+' } else { b'=' },
        0x2F => if shift { b'{' } else { b'[' },
        0x30 => if shift { b'}' } else { b']' },
        0x31 => if shift { b'|' } else { b'\\' },
        0x33 => if shift { b':' } else { b';' },
        0x34 => if shift { b'"' } else { b'\'' },
        0x35 => if shift { b'~' } else { b'`' },
        0x36 => if shift { b'<' } else { b',' },
        0x37 => if shift { b'>' } else { b'.' },
        0x38 => if shift { b'?' } else { b'/' },
        _ => 0,
    }
}
