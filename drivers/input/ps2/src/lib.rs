#![no_std]

#[cfg(test)]
extern crate std;

pub mod i8042;
pub mod keyboard;
pub mod mouse;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDevice {
    Keyboard = 1,
    Mouse = 2,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyState {
    Released = 0,
    Pressed = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEvent {
    pub device: InputDevice,
    pub code: u16,
    pub value: i16,
    pub ascii: u8,
    pub modifiers: InputModifiers,
}

impl InputEvent {
    pub const fn key(code: u16, state: KeyState, ascii: u8, modifiers: InputModifiers) -> Self {
        Self {
            device: InputDevice::Keyboard,
            code,
            value: match state {
                KeyState::Released => 0,
                KeyState::Pressed => 1,
            },
            ascii,
            modifiers,
        }
    }

    pub const fn mouse(code: u16, value: i16) -> Self {
        Self {
            device: InputDevice::Mouse,
            code,
            value,
            ascii: 0,
            modifiers: InputModifiers {
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            },
        }
    }
}
