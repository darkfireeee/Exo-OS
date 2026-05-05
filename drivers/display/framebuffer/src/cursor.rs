#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Cursor {
    pub const fn new(cols: usize, rows: usize) -> Self {
        Self {
            col: 0,
            row: 0,
            cols,
            rows,
        }
    }

    pub fn carriage_return(&mut self) {
        self.col = 0;
    }

    pub fn newline(&mut self) -> bool {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
            false
        } else {
            true
        }
    }

    pub fn advance(&mut self) -> bool {
        self.col += 1;
        if self.col >= self.cols {
            self.newline()
        } else {
            false
        }
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        }
    }
}
