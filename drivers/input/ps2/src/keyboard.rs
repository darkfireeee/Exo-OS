use crate::{InputEvent, InputModifiers, KeyState};

pub const KEY_ENTER: u16 = 0x0028;
pub const KEY_BACKSPACE: u16 = 0x002a;
pub const KEY_TAB: u16 = 0x002b;
pub const KEY_ESCAPE: u16 = 0x0029;
pub const KEY_SPACE: u16 = 0x002c;
pub const KEY_LEFT_SHIFT: u16 = 0x00e1;
pub const KEY_RIGHT_SHIFT: u16 = 0x00e5;
pub const KEY_LEFT_CTRL: u16 = 0x00e0;
pub const KEY_LEFT_ALT: u16 = 0x00e2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ScancodeSet {
    Set1,
    #[default]
    Set2,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ps2Keyboard {
    scancode_set: ScancodeSet,
    release_next: bool,
    extended_next: bool,
    modifiers: InputModifiers,
}

impl Ps2Keyboard {
    pub const fn new() -> Self {
        Self {
            scancode_set: ScancodeSet::Set2,
            release_next: false,
            extended_next: false,
            modifiers: InputModifiers {
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            },
        }
    }

    pub const fn new_set1() -> Self {
        Self {
            scancode_set: ScancodeSet::Set1,
            release_next: false,
            extended_next: false,
            modifiers: InputModifiers {
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            },
        }
    }

    pub fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        match byte {
            0xE0 => {
                self.extended_next = true;
                None
            }
            0xF0 => {
                self.scancode_set = ScancodeSet::Set2;
                self.release_next = true;
                None
            }
            _ => {
                let mut released = self.release_next;
                let extended = self.extended_next;
                self.release_next = false;
                self.extended_next = false;

                let code = match self.scancode_set {
                    ScancodeSet::Set1 => {
                        released |= byte & 0x80 != 0;
                        map_set1_to_hid(byte & 0x7f, extended)?
                    }
                    ScancodeSet::Set2 => map_set2_to_hid(byte, extended)?,
                };
                let state = if released {
                    KeyState::Released
                } else {
                    KeyState::Pressed
                };
                self.update_modifiers(code, state);
                let ascii = if state == KeyState::Pressed {
                    hid_to_ascii(code, self.modifiers.shift, self.modifiers.ctrl)
                } else {
                    0
                };
                Some(InputEvent::key(code, state, ascii, self.modifiers))
            }
        }
    }

    fn update_modifiers(&mut self, code: u16, state: KeyState) {
        let pressed = state == KeyState::Pressed;
        match code {
            KEY_LEFT_SHIFT | KEY_RIGHT_SHIFT => self.modifiers.shift = pressed,
            KEY_LEFT_CTRL => self.modifiers.ctrl = pressed,
            KEY_LEFT_ALT => self.modifiers.alt = pressed,
            _ => {}
        }
    }
}

pub fn map_set1_to_hid(scancode: u8, extended: bool) -> Option<u16> {
    if extended {
        return match scancode {
            0x1c => Some(KEY_ENTER),
            0x1d => Some(KEY_LEFT_CTRL),
            0x38 => Some(KEY_LEFT_ALT),
            0x48 => Some(0x0052), // up
            0x50 => Some(0x0051), // down
            0x4b => Some(0x0050), // left
            0x4d => Some(0x004f), // right
            0x53 => Some(0x004c), // delete
            0x52 => Some(0x004a), // insert
            0x47 => Some(0x004a), // home
            0x4f => Some(0x004d), // end
            0x49 => Some(0x004b), // page up
            0x51 => Some(0x004e), // page down
            _ => None,
        };
    }

    match scancode {
        0x1e => Some(0x0004),
        0x30 => Some(0x0005),
        0x2e => Some(0x0006),
        0x20 => Some(0x0007),
        0x12 => Some(0x0008),
        0x21 => Some(0x0009),
        0x22 => Some(0x000a),
        0x23 => Some(0x000b),
        0x17 => Some(0x000c),
        0x24 => Some(0x000d),
        0x25 => Some(0x000e),
        0x26 => Some(0x000f),
        0x32 => Some(0x0010),
        0x31 => Some(0x0011),
        0x18 => Some(0x0012),
        0x19 => Some(0x0013),
        0x10 => Some(0x0014),
        0x13 => Some(0x0015),
        0x1f => Some(0x0016),
        0x14 => Some(0x0017),
        0x16 => Some(0x0018),
        0x2f => Some(0x0019),
        0x11 => Some(0x001a),
        0x2d => Some(0x001b),
        0x15 => Some(0x001c),
        0x2c => Some(0x001d),
        0x02 => Some(0x001e),
        0x03 => Some(0x001f),
        0x04 => Some(0x0020),
        0x05 => Some(0x0021),
        0x06 => Some(0x0022),
        0x07 => Some(0x0023),
        0x08 => Some(0x0024),
        0x09 => Some(0x0025),
        0x0a => Some(0x0026),
        0x0b => Some(0x0027),
        0x1c => Some(KEY_ENTER),
        0x01 => Some(KEY_ESCAPE),
        0x0e => Some(KEY_BACKSPACE),
        0x0f => Some(KEY_TAB),
        0x39 => Some(KEY_SPACE),
        0x0c => Some(0x002d),
        0x0d => Some(0x002e),
        0x1a => Some(0x002f),
        0x1b => Some(0x0030),
        0x2b => Some(0x0031),
        0x27 => Some(0x0033),
        0x28 => Some(0x0034),
        0x29 => Some(0x0035),
        0x33 => Some(0x0036),
        0x34 => Some(0x0037),
        0x35 => Some(0x0038),
        0x2a => Some(KEY_LEFT_SHIFT),
        0x36 => Some(KEY_RIGHT_SHIFT),
        0x1d => Some(KEY_LEFT_CTRL),
        0x38 => Some(KEY_LEFT_ALT),
        _ => None,
    }
}

pub fn map_set2_to_hid(scancode: u8, extended: bool) -> Option<u16> {
    if extended {
        return match scancode {
            0x5a => Some(KEY_ENTER),
            0x14 => Some(KEY_LEFT_CTRL),
            0x11 => Some(KEY_LEFT_ALT),
            0x75 => Some(0x0052), // up
            0x72 => Some(0x0051), // down
            0x6b => Some(0x0050), // left
            0x74 => Some(0x004f), // right
            0x71 => Some(0x004c), // delete
            0x70 => Some(0x004a), // insert
            0x6c => Some(0x004a), // home
            0x69 => Some(0x004d), // end
            0x7d => Some(0x004b), // page up
            0x7a => Some(0x004e), // page down
            _ => None,
        };
    }

    match scancode {
        0x1c => Some(0x0004),
        0x32 => Some(0x0005),
        0x21 => Some(0x0006),
        0x23 => Some(0x0007),
        0x24 => Some(0x0008),
        0x2b => Some(0x0009),
        0x34 => Some(0x000a),
        0x33 => Some(0x000b),
        0x43 => Some(0x000c),
        0x3b => Some(0x000d),
        0x42 => Some(0x000e),
        0x4b => Some(0x000f),
        0x3a => Some(0x0010),
        0x31 => Some(0x0011),
        0x44 => Some(0x0012),
        0x4d => Some(0x0013),
        0x15 => Some(0x0014),
        0x2d => Some(0x0015),
        0x1b => Some(0x0016),
        0x2c => Some(0x0017),
        0x3c => Some(0x0018),
        0x2a => Some(0x0019),
        0x1d => Some(0x001a),
        0x22 => Some(0x001b),
        0x35 => Some(0x001c),
        0x1a => Some(0x001d),
        0x16 => Some(0x001e),
        0x1e => Some(0x001f),
        0x26 => Some(0x0020),
        0x25 => Some(0x0021),
        0x2e => Some(0x0022),
        0x36 => Some(0x0023),
        0x3d => Some(0x0024),
        0x3e => Some(0x0025),
        0x46 => Some(0x0026),
        0x45 => Some(0x0027),
        0x5a => Some(KEY_ENTER),
        0x76 => Some(KEY_ESCAPE),
        0x66 => Some(KEY_BACKSPACE),
        0x0d => Some(KEY_TAB),
        0x29 => Some(KEY_SPACE),
        0x4e => Some(0x002d),
        0x55 => Some(0x002e),
        0x54 => Some(0x002f),
        0x5b => Some(0x0030),
        0x5d => Some(0x0031),
        0x4c => Some(0x0033),
        0x52 => Some(0x0034),
        0x0e => Some(0x0035),
        0x41 => Some(0x0036),
        0x49 => Some(0x0037),
        0x4a => Some(0x0038),
        0x12 => Some(KEY_LEFT_SHIFT),
        0x59 => Some(KEY_RIGHT_SHIFT),
        0x14 => Some(KEY_LEFT_CTRL),
        0x11 => Some(KEY_LEFT_ALT),
        _ => None,
    }
}

pub fn hid_to_ascii(code: u16, shift: bool, ctrl: bool) -> u8 {
    let c = match code {
        0x0004..=0x001d => {
            let base = if shift { b'A' } else { b'a' };
            base + (code as u8 - 0x04)
        }
        0x001e => {
            if shift {
                b'!'
            } else {
                b'1'
            }
        }
        0x001f => {
            if shift {
                b'@'
            } else {
                b'2'
            }
        }
        0x0020 => {
            if shift {
                b'#'
            } else {
                b'3'
            }
        }
        0x0021 => {
            if shift {
                b'$'
            } else {
                b'4'
            }
        }
        0x0022 => {
            if shift {
                b'%'
            } else {
                b'5'
            }
        }
        0x0023 => {
            if shift {
                b'^'
            } else {
                b'6'
            }
        }
        0x0024 => {
            if shift {
                b'&'
            } else {
                b'7'
            }
        }
        0x0025 => {
            if shift {
                b'*'
            } else {
                b'8'
            }
        }
        0x0026 => {
            if shift {
                b'('
            } else {
                b'9'
            }
        }
        0x0027 => {
            if shift {
                b')'
            } else {
                b'0'
            }
        }
        KEY_ENTER => b'\n',
        KEY_BACKSPACE => 0x08,
        KEY_TAB => b'\t',
        KEY_SPACE => b' ',
        0x002d => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        0x002e => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        0x002f => {
            if shift {
                b'{'
            } else {
                b'['
            }
        }
        0x0030 => {
            if shift {
                b'}'
            } else {
                b']'
            }
        }
        0x0031 => {
            if shift {
                b'|'
            } else {
                b'\\'
            }
        }
        0x0033 => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        0x0034 => {
            if shift {
                b'"'
            } else {
                b'\''
            }
        }
        0x0035 => {
            if shift {
                b'~'
            } else {
                b'`'
            }
        }
        0x0036 => {
            if shift {
                b'<'
            } else {
                b','
            }
        }
        0x0037 => {
            if shift {
                b'>'
            } else {
                b'.'
            }
        }
        0x0038 => {
            if shift {
                b'?'
            } else {
                b'/'
            }
        }
        _ => 0,
    };

    if ctrl && c.is_ascii_alphabetic() {
        (c.to_ascii_lowercase() - b'a') + 1
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_plain_and_release_key() {
        let mut kb = Ps2Keyboard::new();
        let press = kb.feed(0x1c).unwrap();
        assert_eq!(press.ascii, b'a');
        assert_eq!(press.value, 1);
        assert!(kb.feed(0xf0).is_none());
        let release = kb.feed(0x1c).unwrap();
        assert_eq!(release.code, 0x0004);
        assert_eq!(release.value, 0);
    }

    #[test]
    fn shift_changes_ascii_until_released() {
        let mut kb = Ps2Keyboard::new();
        assert_eq!(kb.feed(0x12).unwrap().code, KEY_LEFT_SHIFT);
        assert_eq!(kb.feed(0x1c).unwrap().ascii, b'A');
        assert!(kb.feed(0xf0).is_none());
        assert_eq!(kb.feed(0x12).unwrap().value, 0);
        assert_eq!(kb.feed(0x1c).unwrap().ascii, b'a');
    }

    #[test]
    fn ctrl_letter_emits_control_code() {
        let mut kb = Ps2Keyboard::new();
        assert_eq!(kb.feed(0x14).unwrap().code, KEY_LEFT_CTRL);
        assert_eq!(kb.feed(0x21).unwrap().ascii, 3);
    }

    #[test]
    fn decodes_translated_set1_key() {
        let mut kb = Ps2Keyboard::new_set1();
        let press = kb.feed(0x1e).unwrap();
        assert_eq!(press.ascii, b'a');
        assert_eq!(press.value, 1);
        let release = kb.feed(0x9e).unwrap();
        assert_eq!(release.code, 0x0004);
        assert_eq!(release.value, 0);
    }
}
