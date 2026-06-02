pub const LINE_MAX: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    Interrupt,
    EndOfFile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEvent {
    Echo(u8),
    Backspace,
    ClearScreen,
    LineReady { len: usize },
    Signal(Signal),
}

#[derive(Clone, Debug)]
pub struct LineDiscipline {
    buf: [u8; LINE_MAX],
    len: usize,
    canonical: bool,
    echo: bool,
}

impl Default for LineDiscipline {
    fn default() -> Self {
        Self::new()
    }
}

impl LineDiscipline {
    pub const fn new() -> Self {
        Self {
            buf: [0; LINE_MAX],
            len: 0,
            canonical: true,
            echo: true,
        }
    }

    pub fn set_canonical(&mut self, enabled: bool) {
        self.canonical = enabled;
    }

    pub fn set_echo(&mut self, enabled: bool) {
        self.echo = enabled;
    }

    pub fn line(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn take_line<'a>(&mut self, out: &'a mut [u8]) -> &'a [u8] {
        let n = core::cmp::min(out.len(), self.len);
        out[..n].copy_from_slice(&self.buf[..n]);
        self.len = 0;
        &out[..n]
    }

    pub fn input_byte(&mut self, byte: u8) -> Option<LineEvent> {
        match byte {
            3 => {
                self.len = 0;
                Some(LineEvent::Signal(Signal::Interrupt))
            }
            4 => {
                if self.len == 0 {
                    Some(LineEvent::Signal(Signal::EndOfFile))
                } else {
                    self.finish_line()
                }
            }
            b'\r' | b'\n' if self.canonical => {
                if self.len < LINE_MAX {
                    self.buf[self.len] = b'\n';
                    self.len += 1;
                }
                self.finish_line()
            }
            0x08 | 0x7f if self.canonical => {
                if self.len > 0 {
                    self.len -= 1;
                    Some(LineEvent::Backspace)
                } else {
                    None
                }
            }
            0x0c if self.canonical => Some(LineEvent::ClearScreen),
            byte => {
                if self.len < LINE_MAX {
                    self.buf[self.len] = byte;
                    self.len += 1;
                    if !self.canonical {
                        return Some(LineEvent::LineReady { len: self.len });
                    }
                    if self.echo {
                        Some(LineEvent::Echo(byte))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        }
    }

    fn finish_line(&self) -> Option<LineEvent> {
        Some(LineEvent::LineReady { len: self.len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_line_collects_until_newline() {
        let mut ld = LineDiscipline::new();
        assert_eq!(ld.input_byte(b'a'), Some(LineEvent::Echo(b'a')));
        assert_eq!(ld.input_byte(b'\n'), Some(LineEvent::LineReady { len: 2 }));
        assert_eq!(ld.line(), b"a\n");
    }

    #[test]
    fn backspace_removes_previous_byte() {
        let mut ld = LineDiscipline::new();
        let _ = ld.input_byte(b'a');
        let _ = ld.input_byte(b'b');
        assert_eq!(ld.input_byte(0x08), Some(LineEvent::Backspace));
        assert_eq!(ld.line(), b"a");
    }

    #[test]
    fn ctrl_c_reports_interrupt_and_clears() {
        let mut ld = LineDiscipline::new();
        let _ = ld.input_byte(b'a');
        assert_eq!(ld.input_byte(3), Some(LineEvent::Signal(Signal::Interrupt)));
        assert_eq!(ld.line(), b"");
    }

    #[test]
    fn ctrl_l_clears_without_entering_line() {
        let mut ld = LineDiscipline::new();
        let _ = ld.input_byte(b'p');
        assert_eq!(ld.input_byte(0x0c), Some(LineEvent::ClearScreen));
        assert_eq!(ld.line(), b"p");
    }
}
