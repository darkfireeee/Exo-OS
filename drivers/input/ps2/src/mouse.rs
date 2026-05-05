use crate::InputEvent;

pub const MOUSE_LEFT: u16 = 0x0100;
pub const MOUSE_RIGHT: u16 = 0x0101;
pub const MOUSE_MIDDLE: u16 = 0x0102;
pub const MOUSE_DX: u16 = 0x0110;
pub const MOUSE_DY: u16 = 0x0111;

#[derive(Clone, Copy, Debug, Default)]
pub struct Ps2Mouse {
    packet: [u8; 3],
    len: usize,
}

impl Ps2Mouse {
    pub const fn new() -> Self {
        Self {
            packet: [0; 3],
            len: 0,
        }
    }

    pub fn feed(&mut self, byte: u8, out: &mut [Option<InputEvent>; 5]) -> usize {
        if self.len == 0 && byte & 0x08 == 0 {
            return 0;
        }
        self.packet[self.len] = byte;
        self.len += 1;
        if self.len < 3 {
            return 0;
        }
        self.len = 0;

        let buttons = self.packet[0];
        let dx = sign_extend(self.packet[1], buttons & 0x10 != 0);
        let dy = -sign_extend(self.packet[2], buttons & 0x20 != 0);
        let mut n = 0usize;
        out[n] = Some(InputEvent::mouse(MOUSE_DX, dx));
        n += 1;
        out[n] = Some(InputEvent::mouse(MOUSE_DY, dy));
        n += 1;
        out[n] = Some(InputEvent::mouse(MOUSE_LEFT, (buttons & 1) as i16));
        n += 1;
        out[n] = Some(InputEvent::mouse(MOUSE_RIGHT, ((buttons >> 1) & 1) as i16));
        n += 1;
        out[n] = Some(InputEvent::mouse(MOUSE_MIDDLE, ((buttons >> 2) & 1) as i16));
        n + 1
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
        assert_eq!(mouse.feed(1, &mut out), 5);
        assert_eq!(out[0].unwrap().value, 5);
        assert_eq!(out[1].unwrap().value, -1);
        assert_eq!(out[2].unwrap().value, 1);
    }
}
