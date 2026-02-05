//! Entrées/sorties standard (stdin, stdout, stderr)

use crate::error::IoError;
use crate::syscall::io::{read, write};
use super::traits::{Read, Write};
use core::fmt;

/// Entrée standard
pub struct Stdin {
    _private: (),
}

impl Stdin {
    /// Crée une nouvelle instance (usage interne)
    const fn new() -> Self {
        Self { _private: () }
    }

    /// Lit une ligne dans un buffer
    pub fn read_line(&self, buf: &mut [u8]) -> Result<usize, IoError> {
        let mut total = 0;
        for i in 0..buf.len() {
            let mut byte = [0u8];
            match unsafe { read(0, &mut byte) } {
                Ok(0) => break,
                Ok(_) => {
                    buf[i] = byte[0];
                    total += 1;
                    if byte[0] == b'\n' {
                        break;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(total)
    }
}

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        if buf.is_empty() {
            return Ok(0);
        }
        unsafe { read(0, buf) }
    }
}

/// Sortie standard
pub struct Stdout {
    _private: (),
}

impl Stdout {
    /// Crée une nouvelle instance (usage interne)
    const fn new() -> Self {
        Self { _private: () }
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        if buf.is_empty() {
            return Ok(0);
        }
        unsafe { write(1, buf) }
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

/// Erreur standard
pub struct Stderr {
    _private: (),
}

impl Stderr {
    /// Crée une nouvelle instance (usage interne)
    const fn new() -> Self {
        Self { _private: () }
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        if buf.is_empty() {
            return Ok(0);
        }
        unsafe { write(2, buf) }
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

impl fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

/// Retourne un handle sur l'entrée standard
pub const fn stdin() -> Stdin {
    Stdin::new()
}

/// Retourne un handle sur la sortie standard
pub const fn stdout() -> Stdout {
    Stdout::new()
}

/// Retourne un handle sur l'erreur standard
pub const fn stderr() -> Stderr {
    Stderr::new()
}

/// Écrit sur stdout avec macro print!
#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    let _ = <Stdout as fmt::Write>::write_fmt(&mut stdout(), args);
}

/// Écrit sur stderr avec macro eprint!
#[doc(hidden)]
pub fn _eprint(args: fmt::Arguments<'_>) {
    let _ = <Stderr as fmt::Write>::write_fmt(&mut stderr(), args);
}
