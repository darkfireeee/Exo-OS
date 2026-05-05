#![no_std]

#[cfg(test)]
extern crate std;

pub const VGA_COLS: usize = 80;
pub const VGA_ROWS: usize = 25;
pub const VGA_CELLS: usize = VGA_COLS * VGA_ROWS;
pub const VGA_TEXT_PHYS: usize = 0xb8000;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorCode(pub u8);

impl ColorCode {
    pub const LIGHT_GREY_ON_BLACK: Self = Self(0x07);
    pub const GREEN_ON_BLACK: Self = Self(0x02);
    pub const RED_ON_BLACK: Self = Self(0x04);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VgaCell {
    pub ascii: u8,
    pub color: ColorCode,
}

impl VgaCell {
    pub const fn blank(color: ColorCode) -> Self {
        Self { ascii: b' ', color }
    }
}

pub struct VgaTextBuffer<'a> {
    cells: &'a mut [VgaCell],
    row: usize,
    col: usize,
    color: ColorCode,
}

impl<'a> VgaTextBuffer<'a> {
    pub fn new(cells: &'a mut [VgaCell]) -> Self {
        assert!(cells.len() >= VGA_CELLS);
        Self {
            cells,
            row: 0,
            col: 0,
            color: ColorCode::LIGHT_GREY_ON_BLACK,
        }
    }

    pub fn set_color(&mut self, color: ColorCode) {
        self.color = color;
    }

    pub fn clear(&mut self) {
        for cell in self.cells.iter_mut().take(VGA_CELLS) {
            *cell = VgaCell::blank(self.color);
        }
        self.row = 0;
        self.col = 0;
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            0x08 | 0x7f => self.backspace(),
            byte => {
                if self.col >= VGA_COLS {
                    self.newline();
                }
                let idx = self.row * VGA_COLS + self.col;
                self.cells[idx] = VgaCell {
                    ascii: printable(byte),
                    color: self.color,
                };
                self.col += 1;
            }
        }
    }

    pub fn write_all(&mut self, data: &[u8]) {
        for &byte in data {
            self.write_byte(byte);
        }
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < VGA_ROWS {
            self.row += 1;
        } else {
            self.scroll_up();
        }
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let idx = self.row * VGA_COLS + self.col;
            self.cells[idx] = VgaCell::blank(self.color);
        }
    }

    fn scroll_up(&mut self) {
        for row in 1..VGA_ROWS {
            for col in 0..VGA_COLS {
                self.cells[(row - 1) * VGA_COLS + col] = self.cells[row * VGA_COLS + col];
            }
        }
        for col in 0..VGA_COLS {
            self.cells[(VGA_ROWS - 1) * VGA_COLS + col] = VgaCell::blank(self.color);
        }
    }
}

fn printable(byte: u8) -> u8 {
    if byte.is_ascii_graphic() || byte == b' ' {
        byte
    } else {
        0xfe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_text_and_cursor() {
        let mut cells = [VgaCell::blank(ColorCode::LIGHT_GREY_ON_BLACK); VGA_CELLS];
        let mut vga = VgaTextBuffer::new(&mut cells);
        vga.write_all(b"exo\nos");
        let cursor = vga.cursor();
        drop(vga);
        assert_eq!(cells[0].ascii, b'e');
        assert_eq!(cells[VGA_COLS].ascii, b'o');
        assert_eq!(cursor, (1, 2));
    }

    #[test]
    fn scrolls_last_line() {
        let mut cells = [VgaCell::blank(ColorCode::LIGHT_GREY_ON_BLACK); VGA_CELLS];
        let mut vga = VgaTextBuffer::new(&mut cells);
        for _ in 0..VGA_ROWS {
            vga.write_all(b"x\n");
        }
        assert_eq!(vga.cursor(), (VGA_ROWS - 1, 0));
        assert_eq!(cells[0].ascii, b'x');
    }
}
