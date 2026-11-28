use crate::InputEvent;

pub const MOUSE_LEFT: u16 = 0x0100;
pub const MOUSE_RIGHT: u16 = 0x0101;
pub const MOUSE_MIDDLE: u16 = 0x0102;
pub const MOUSE_DX: u16 = 0x0110;
pub const MOUSE_DY: u16 = 0x0111;
pub const MOUSE_WHEEL: u16 = 0x0112;

#[derive(Clone, Copy, Debug, Default)]
pub struct Ps2Mouse {
    packet: [u8; 4],
    len: usize,
    has_wheel: bool,
    prev_buttons: u8,
    buttons_initialized: bool,
}

impl Ps2Mouse {
    pub const fn new() -> Self {
        Self::with_wheel(false)
    }

    pub const fn with_wheel(has_wheel: bool) -> Self {
        Self {
            packet: [0; 4],
            len: 0,
            has_wheel,
            prev_buttons: 0,
            buttons_initialized: false,
        }
    }

    pub fn feed(&mut self, byte: u8, out: &mut [Option<InputEvent>; 5]) -> usize {
        if self.len == 0 && ((byte & 0x08) == 0 || (byte & 0xC0) != 0) {
            return 0;
        }
        let pkt_size = if self.has_wheel { 4 } else { 3 };
        self.packet[self.len] = byte;
        self.len += 1;
        if self.len < pkt_size {
            return 0;
        }
        self.len = 0;

        let buttons = self.packet[0];
        let dx = sign_extend(self.packet[1], buttons & 0x10 != 0);
        let dy = -sign_extend(self.packet[2], buttons & 0x20 != 0);
        let dz = if self.has_wheel {
            // IntelliMouse ID 3: signed 8-bit Z-delta; ID 4: signed 4-bit low nibble
            let raw = self.packet[3];
            let s4 = ((raw & 0x0F) as i8) << 4 >> 4;
            if (raw & 0xF0) == 0 || (raw & 0xF0) == 0xF0 {
                (raw as i8) as i16
            } else {
                s4 as i16
            }
        } else {
            0
        };
        *out = [None; 5];
        let mut n = 0usize;
        if dx != 0 && n < out.len() {
            out[n] = Some(InputEvent::mouse(MOUSE_DX, dx));
            n += 1;
        }
        if dy != 0 && n < out.len() {
            out[n] = Some(InputEvent::mouse(MOUSE_DY, dy));
            n += 1;
        }
        if dz != 0 && n < out.len() {
            out[n] = Some(InputEvent::mouse(MOUSE_WHEEL, dz));
            n += 1;
        }
        let btn_bits = buttons & 0x07;
        for (bit, code) in [(0u8, MOUSE_LEFT), (1u8, MOUSE_RIGHT), (2u8, MOUSE_MIDDLE)] {
            let cur = (btn_bits >> bit) & 1;
            let prev = (self.prev_buttons >> bit) & 1;
            if (cur != prev || (!self.buttons_initialized && cur != 0)) && n < out.len() {
                out[n] = Some(InputEvent::mouse(code, cur as i16));
                n += 1;
            }
        }
        self.prev_buttons = btn_bits;
        self.buttons_initialized = true;
        n
    }
}

fn sign_extend(byte: u8, negative: bool) -> i16 {
    if negative {
        (byte as i16) - 256
    } else {
        byte as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_three_byte_packet() {
        let mut mouse = Ps2Mouse::new();
        let mut out = [None; 5];
        assert_eq!(mouse.feed(0x09, &mut out), 0);
        assert_eq!(mouse.feed(5, &mut out), 0);
        assert_eq!(mouse.feed(1, &mut out), 3);
        assert_eq!(out[0].unwrap().code, MOUSE_DX);
        assert_eq!(out[0].unwrap().value, 5);
        assert_eq!(out[1].unwrap().code, MOUSE_DY);
        assert_eq!(out[1].unwrap().value, -1);
        assert_eq!(out[2].unwrap().code, MOUSE_LEFT);
        assert_eq!(out[2].unwrap().value, 1);

        // Next packet with same held button and no movement emits 0 events.
        assert_eq!(mouse.feed(0x09, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);

        // Button release with zero movement emits only the release event.
        assert_eq!(mouse.feed(0x08, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 1);
        assert_eq!(out[0].unwrap().code, MOUSE_LEFT);
        assert_eq!(out[0].unwrap().value, 0);
    }

    #[test]
    fn decodes_four_byte_intellimouse_wheel_packet() {
        let mut mouse = Ps2Mouse::with_wheel(true);
        let mut out = [None; 5];
        assert_eq!(mouse.feed(0x08, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);
        assert_eq!(mouse.feed(1, &mut out), 1);
        assert_eq!(out[0].unwrap().code, MOUSE_WHEEL);
        assert_eq!(out[0].unwrap().value, 1);

        assert_eq!(mouse.feed(0x08, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);
        assert_eq!(mouse.feed(0, &mut out), 0);
        assert_eq!(mouse.feed(0xFF, &mut out), 1);
        assert_eq!(out[0].unwrap().code, MOUSE_WHEEL);
        assert_eq!(out[0].unwrap().value, -1);
    }
}
