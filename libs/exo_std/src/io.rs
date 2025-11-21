// libs/exo_std/src/io.rs
use core::fmt;

/// Trait pour la lecture
pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> crate::Result<usize>;
}

/// Trait pour l'écriture
pub trait Write {
    fn write(&mut self, buf: &[u8]) -> crate::Result<usize>;
    fn flush(&mut self) -> crate::Result<()>;
}

/// Entrée standard
pub struct Stdin;

/// Sortie standard
pub struct Stdout;

/// Erreur standard
pub struct Stderr;

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> crate::Result<usize> {
        // TODO: Implement syscall
        Ok(buf.len())
    }

    fn flush(&mut self) -> crate::Result<()> {
        Ok(())
    }
}

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _ = self.write(s.as_bytes());
        Ok(())
    }
}

pub fn stdout() -> Stdout {
    Stdout
}
