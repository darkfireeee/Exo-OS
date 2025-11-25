//! Driver clavier PS/2 avec traduction scancode → ASCII
//! 
//! Support:
//! - Scancode Set 1 (standard PC)
//! - QWERTY et AZERTY
//! - Shift, Caps Lock, Ctrl, Alt
//! - Buffer circulaire pour les touches

use spin::Mutex;

/// Layout clavier
#[derive(Copy, Clone, PartialEq)]
pub enum KeyboardLayout {
    Qwerty,
    Azerty,
}

/// État des touches modificatrices
#[derive(Copy, Clone)]
struct ModifierState {
    shift_pressed: bool,
    ctrl_pressed: bool,
    alt_pressed: bool,
    caps_lock: bool,
}

impl ModifierState {
    const fn new() -> Self {
        ModifierState {
            shift_pressed: false,
            ctrl_pressed: false,
            alt_pressed: false,
            caps_lock: false,
        }
    }
}

/// Buffer circulaire pour les touches
const BUFFER_SIZE: usize = 128;

struct KeyboardBuffer {
    buffer: [char; BUFFER_SIZE],
    read_pos: usize,
    write_pos: usize,
    count: usize,
}

impl KeyboardBuffer {
    const fn new() -> Self {
        KeyboardBuffer {
            buffer: ['\0'; BUFFER_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
        }
    }

    fn push(&mut self, c: char) {
        if self.count < BUFFER_SIZE {
            self.buffer[self.write_pos] = c;
            self.write_pos = (self.write_pos + 1) % BUFFER_SIZE;
            self.count += 1;
        }
    }

    fn pop(&mut self) -> Option<char> {
        if self.count > 0 {
            let c = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % BUFFER_SIZE;
            self.count -= 1;
            Some(c)
        } else {
            None
        }
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// État global du clavier
static KEYBOARD_STATE: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());

struct KeyboardState {
    modifiers: ModifierState,
    buffer: KeyboardBuffer,
    layout: KeyboardLayout,
}

impl KeyboardState {
    const fn new() -> Self {
        KeyboardState {
            modifiers: ModifierState::new(),
            buffer: KeyboardBuffer::new(),
            layout: KeyboardLayout::Qwerty,
        }
    }
}

/// Configure le layout clavier
pub fn set_layout(layout: KeyboardLayout) {
    KEYBOARD_STATE.lock().layout = layout;
}

/// Traite un scancode et retourne le caractère correspondant
pub fn process_scancode(scancode: u8) -> Option<char> {
    let mut state = KEYBOARD_STATE.lock();
    
    // Touches de release (bit 7 = 1)
    let is_release = (scancode & 0x80) != 0;
    let key_code = scancode & 0x7F;
    
    // Gérer les modificateurs
    match key_code {
        0x2A | 0x36 => { // Left/Right Shift
            state.modifiers.shift_pressed = !is_release;
            return None;
        }
        0x1D => { // Ctrl
            state.modifiers.ctrl_pressed = !is_release;
            return None;
        }
        0x38 => { // Alt
            state.modifiers.alt_pressed = !is_release;
            return None;
        }
        0x3A => { // Caps Lock (toggle on press)
            if !is_release {
                state.modifiers.caps_lock = !state.modifiers.caps_lock;
            }
            return None;
        }
        _ => {}
    }
    
    // Ignorer les releases pour les touches normales
    if is_release {
        return None;
    }
    
    // Traduire en caractère
    let c = match state.layout {
        KeyboardLayout::Qwerty => scancode_to_char_qwerty(key_code, &state.modifiers),
        KeyboardLayout::Azerty => scancode_to_char_azerty(key_code, &state.modifiers),
    };
    
    // Ajouter au buffer si c'est un caractère valide
    if let Some(ch) = c {
        state.buffer.push(ch);
    }
    
    c
}

/// Lit un caractère depuis le buffer
pub fn read_char() -> Option<char> {
    KEYBOARD_STATE.lock().buffer.pop()
}

/// Vérifie si le buffer est vide
pub fn has_char() -> bool {
    !KEYBOARD_STATE.lock().buffer.is_empty()
}

/// Traduction scancode → char (QWERTY)
fn scancode_to_char_qwerty(scancode: u8, modifiers: &ModifierState) -> Option<char> {
    let shift = modifiers.shift_pressed;
    let caps = modifiers.caps_lock;
    
    // Appliquer caps lock uniquement aux lettres
    let apply_caps = |c: char| -> char {
        if c.is_ascii_alphabetic() {
            if (shift && !caps) || (!shift && caps) {
                c.to_ascii_uppercase()
            } else {
                c
            }
        } else if shift {
            c
        } else {
            c.to_ascii_lowercase()
        }
    };
    
    let c = match scancode {
        // Chiffres
        0x02 => if shift { '!' } else { '1' },
        0x03 => if shift { '@' } else { '2' },
        0x04 => if shift { '#' } else { '3' },
        0x05 => if shift { '$' } else { '4' },
        0x06 => if shift { '%' } else { '5' },
        0x07 => if shift { '^' } else { '6' },
        0x08 => if shift { '&' } else { '7' },
        0x09 => if shift { '*' } else { '8' },
        0x0A => if shift { '(' } else { '9' },
        0x0B => if shift { ')' } else { '0' },
        
        // Lettres (ligne QWERTY)
        0x10 => 'q', 0x11 => 'w', 0x12 => 'e', 0x13 => 'r', 0x14 => 't',
        0x15 => 'y', 0x16 => 'u', 0x17 => 'i', 0x18 => 'o', 0x19 => 'p',
        
        // Lettres (ligne ASDF)
        0x1E => 'a', 0x1F => 's', 0x20 => 'd', 0x21 => 'f', 0x22 => 'g',
        0x23 => 'h', 0x24 => 'j', 0x25 => 'k', 0x26 => 'l',
        
        // Lettres (ligne ZXCV)
        0x2C => 'z', 0x2D => 'x', 0x2E => 'c', 0x2F => 'v', 0x30 => 'b',
        0x31 => 'n', 0x32 => 'm',
        
        // Ponctuation
        0x0C => if shift { '_' } else { '-' },
        0x0D => if shift { '+' } else { '=' },
        0x1A => if shift { '{' } else { '[' },
        0x1B => if shift { '}' } else { ']' },
        0x27 => if shift { ':' } else { ';' },
        0x28 => if shift { '"' } else { '\'' },
        0x29 => if shift { '~' } else { '`' },
        0x2B => if shift { '|' } else { '\\' },
        0x33 => if shift { '<' } else { ',' },
        0x34 => if shift { '>' } else { '.' },
        0x35 => if shift { '?' } else { '/' },
        
        // Touches spéciales
        0x39 => ' ',  // Space
        0x1C => '\n', // Enter
        0x0E => '\x08', // Backspace
        0x0F => '\t', // Tab
        
        _ => return None,
    };
    
    Some(apply_caps(c))
}

/// Traduction scancode → char (AZERTY)
fn scancode_to_char_azerty(scancode: u8, modifiers: &ModifierState) -> Option<char> {
    let shift = modifiers.shift_pressed;
    let caps = modifiers.caps_lock;
    
    let apply_caps = |c: char| -> char {
        if c.is_ascii_alphabetic() {
            if (shift && !caps) || (!shift && caps) {
                c.to_ascii_uppercase()
            } else {
                c
            }
        } else if shift {
            c
        } else {
            c.to_ascii_lowercase()
        }
    };
    
    let c = match scancode {
        // Chiffres AZERTY (shift pour avoir les chiffres)
        0x02 => if shift { '1' } else { '&' },
        0x03 => if shift { '2' } else { 'é' },
        0x04 => if shift { '3' } else { '"' },
        0x05 => if shift { '4' } else { '\'' },
        0x06 => if shift { '5' } else { '(' },
        0x07 => if shift { '6' } else { '-' },
        0x08 => if shift { '7' } else { 'è' },
        0x09 => if shift { '8' } else { '_' },
        0x0A => if shift { '9' } else { 'ç' },
        0x0B => if shift { '0' } else { 'à' },
        
        // Lettres (ligne AZERTY)
        0x10 => 'a', 0x11 => 'z', 0x12 => 'e', 0x13 => 'r', 0x14 => 't',
        0x15 => 'y', 0x16 => 'u', 0x17 => 'i', 0x18 => 'o', 0x19 => 'p',
        
        // Lettres (ligne QSDF)
        0x1E => 'q', 0x1F => 's', 0x20 => 'd', 0x21 => 'f', 0x22 => 'g',
        0x23 => 'h', 0x24 => 'j', 0x25 => 'k', 0x26 => 'l',
        
        // Lettres (ligne WXCV)
        0x2C => 'w', 0x2D => 'x', 0x2E => 'c', 0x2F => 'v', 0x30 => 'b',
        0x31 => 'n', 0x32 => 'm',
        
        // Ponctuation AZERTY
        0x0C => if shift { '°' } else { ')' },
        0x0D => if shift { '+' } else { '=' },
        0x1A => if shift { '¨' } else { '^' },
        0x1B => if shift { '£' } else { '$' },
        0x27 => if shift { 'M' } else { 'm' },
        0x28 => if shift { '%' } else { 'ù' },
        0x29 => if shift { '²' } else { '²' },
        0x2B => if shift { 'µ' } else { '*' },
        0x33 => if shift { '?' } else { ',' },
        0x34 => if shift { '.' } else { ';' },
        0x35 => if shift { '/' } else { ':' },
        
        // Touches spéciales
        0x39 => ' ',
        0x1C => '\n',
        0x0E => '\x08',
        0x0F => '\t',
        
        _ => return None,
    };
    
    Some(apply_caps(c))
}

/// Retourne l'état des modificateurs (pour debug)
pub fn get_modifiers() -> (bool, bool, bool, bool) {
    let state = KEYBOARD_STATE.lock();
    (
        state.modifiers.shift_pressed,
        state.modifiers.ctrl_pressed,
        state.modifiers.alt_pressed,
        state.modifiers.caps_lock,
    )
}
