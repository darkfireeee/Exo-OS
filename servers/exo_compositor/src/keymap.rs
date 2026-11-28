//! Live keyboard layout remapper: AZERTY FR, QWERTY US, and BÉPO (ExoOS UI Spec §1.6).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardLayout {
    AzertyFr,
    QwertyUs,
    Bepo,
}

impl KeyboardLayout {
    pub const ALL: [Self; 3] = [Self::AzertyFr, Self::QwertyUs, Self::Bepo];

    pub const fn code(self) -> &'static str {
        match self {
            Self::AzertyFr => "AZERTY",
            Self::QwertyUs => "QWERTY",
            Self::Bepo => "BÉPO",
        }
    }

    pub const fn short(self) -> &'static str {
        match self {
            Self::AzertyFr => "FR",
            Self::QwertyUs => "US",
            Self::Bepo => "BP",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::AzertyFr => Self::QwertyUs,
            Self::QwertyUs => Self::Bepo,
            Self::Bepo => Self::AzertyFr,
        }
    }

    /// Translate USB HID usage `code` + modifiers into a printable `char` for the active layout.
    pub fn translate_char(self, code: u16, fallback_ascii: u8, shift: bool, ctrl: bool) -> Option<char> {
        if ctrl {
            return None;
        }
        match self {
            Self::QwertyUs => {
                if (0x20..=0x7E).contains(&fallback_ascii) {
                    Some(fallback_ascii as char)
                } else {
                    None
                }
            }
            Self::AzertyFr => azerty_char(code, shift).or_else(|| {
                if (0x20..=0x7E).contains(&fallback_ascii) {
                    Some(fallback_ascii as char)
                } else {
                    None
                }
            }),
            Self::Bepo => bepo_char(code, shift).or_else(|| {
                if (0x20..=0x7E).contains(&fallback_ascii) {
                    Some(fallback_ascii as char)
                } else {
                    None
                }
            }),
        }
    }

    /// Normalize logical shortcut letter (`'a'..'z'`) for `Ctrl+A/C/X/V/Z/Y/S/F` under any layout!
    pub fn logical_shortcut_key(self, code: u16) -> Option<char> {
        let ch = match self {
            Self::QwertyUs => match code {
                0x0004..=0x001d => Some((b'a' + (code as u8 - 0x04)) as char),
                _ => None,
            },
            Self::AzertyFr => azerty_char(code, false),
            Self::Bepo => bepo_char(code, false),
        };
        ch.filter(|c| c.is_ascii_alphabetic())
    }
}

fn azerty_char(code: u16, shift: bool) -> Option<char> {
    let pair = match code {
        // Row 1 numbers (French AZERTY requires Shift for digits)
        0x001e => ('&', '1'),
        0x001f => ('é', '2'),
        0x0020 => ('"', '3'),
        0x0021 => ('\'', '4'),
        0x0022 => ('(', '5'),
        0x0023 => ('-', '6'),
        0x0024 => ('è', '7'),
        0x0025 => ('_', '8'),
        0x0026 => ('ç', '9'),
        0x0027 => ('à', '0'),
        0x002d => (')', '°'),
        0x002e => ('=', '+'),
        // Row 2: A Z E R T Y U I O P
        0x0014 => ('a', 'A'),
        0x001a => ('z', 'Z'),
        0x0008 => ('e', 'E'),
        0x0015 => ('r', 'R'),
        0x0017 => ('t', 'T'),
        0x001c => ('y', 'Y'),
        0x0018 => ('u', 'U'),
        0x000c => ('i', 'I'),
        0x0012 => ('o', 'O'),
        0x0013 => ('p', 'P'),
        // Row 3: Q S D F G H J K L M
        0x0004 => ('q', 'Q'),
        0x0016 => ('s', 'S'),
        0x0007 => ('d', 'D'),
        0x0009 => ('f', 'F'),
        0x000a => ('g', 'G'),
        0x000b => ('h', 'H'),
        0x000d => ('j', 'J'),
        0x000e => ('k', 'K'),
        0x000f => ('l', 'L'),
        0x0033 => ('m', 'M'),
        // Row 4: W X C V B N , ; : !
        0x001d => ('w', 'W'),
        0x001b => ('x', 'X'),
        0x0006 => ('c', 'C'),
        0x0019 => ('v', 'V'),
        0x0005 => ('b', 'B'),
        0x0011 => ('n', 'N'),
        0x0010 => (',', '?'),
        0x0036 => (';', '.'),
        0x0037 => (':', '/'),
        0x0038 => ('!', '§'),
        0x002c => (' ', ' '),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

fn bepo_char(code: u16, shift: bool) -> Option<char> {
    let pair = match code {
        0x0014 => ('b', 'B'),
        0x001a => ('é', 'É'),
        0x0008 => ('p', 'P'),
        0x0015 => ('o', 'O'),
        0x0017 => ('è', 'È'),
        0x001c => ('^', '!'),
        0x0018 => ('v', 'V'),
        0x000c => ('d', 'D'),
        0x0012 => ('l', 'L'),
        0x0013 => ('j', 'J'),
        0x0004 => ('a', 'A'),
        0x0016 => ('u', 'U'),
        0x0007 => ('i', 'I'),
        0x0009 => ('e', 'E'),
        0x000a => (',', ';'),
        0x000b => ('c', 'C'),
        0x000d => ('t', 'T'),
        0x000e => ('s', 'S'),
        0x000f => ('r', 'R'),
        0x0033 => ('n', 'N'),
        0x001d => ('à', 'À'),
        0x001b => ('y', 'Y'),
        0x0006 => ('x', 'X'),
        0x0019 => ('.', ':'),
        0x0005 => ('k', 'K'),
        0x0011 => ('\'', '?'),
        0x0010 => ('q', 'Q'),
        0x0036 => ('g', 'G'),
        0x0037 => ('h', 'H'),
        0x0038 => ('f', 'F'),
        0x002c => (' ', ' '),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azerty_and_qwerty_remap_consistently() {
        assert_eq!(KeyboardLayout::AzertyFr.translate_char(0x0014, b'q', false, false), Some('a'));
        assert_eq!(KeyboardLayout::AzertyFr.translate_char(0x0004, b'a', false, false), Some('q'));
        assert_eq!(KeyboardLayout::AzertyFr.logical_shortcut_key(0x0014), Some('a'));
        assert_eq!(KeyboardLayout::QwertyUs.logical_shortcut_key(0x0004), Some('a'));
        assert_eq!(KeyboardLayout::Bepo.translate_char(0x0004, b'a', false, false), Some('a'));
    }
}
