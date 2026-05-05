pub const CLEAR_SCREEN: &[u8] = b"\x1b[2J";
pub const CURSOR_HOME: &[u8] = b"\x1b[H";
pub const ERASE_LINE: &[u8] = b"\x1b[2K";

pub fn cursor_position(row: u16, col: u16, out: &mut [u8; 16]) -> &[u8] {
    let mut n = 0usize;
    out[n] = 0x1b;
    n += 1;
    out[n] = b'[';
    n += 1;
    n += write_u16(row, &mut out[n..]);
    out[n] = b';';
    n += 1;
    n += write_u16(col, &mut out[n..]);
    out[n] = b'H';
    n += 1;
    &out[..n]
}

fn write_u16(mut value: u16, out: &mut [u8]) -> usize {
    if value == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 5];
    let mut len = 0usize;
    while value != 0 {
        tmp[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    for i in 0..len {
        out[i] = tmp[len - 1 - i];
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_cursor_position() {
        let mut out = [0u8; 16];
        assert_eq!(cursor_position(12, 34, &mut out), b"\x1b[12;34H");
    }
}
