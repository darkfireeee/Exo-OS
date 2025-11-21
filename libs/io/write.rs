// libs/exo_std/src/io/write.rs
use super::{Result, SizeResult};
use alloc::vec::Vec;

/// Trait pour les types qui peuvent écrire des octets
pub trait Write {
    /// Écrit des octets depuis le buffer fourni
    fn write(&mut self, buf: &[u8]) -> SizeResult;
    
    /// Vide tous les buffers internes
    fn flush(&mut self) -> Result<()>;
    
    /// Écrit tous les octets du buffer
    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => return Err(super::IoError::WriteZero),
                Ok(n) => buf = &buf[n..],
                Err(ref e) if e.kind() == super::IoError::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    
    /// Écrit un formaté dans le writer
    fn write_fmt(&mut self, fmt: core::fmt::Arguments) -> Result<()> {
        // Utiliser un buffer sur la pile pour les petites chaînes
        let mut buf = [0u8; 128];
        let mut idx = 0;
        
        struct BufWriter<'a> {
            buf: &'a mut [u8],
            idx: usize,
        }
        
        impl<'a> core::fmt::Write for BufWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                if self.idx + s.len() <= self.buf.len() {
                    self.buf[self.idx..self.idx + s.len()].copy_from_slice(s.as_bytes());
                    self.idx += s.len();
                    Ok(())
                } else {
                    Err(core::fmt::Error)
                }
            }
        }
        
        let mut writer = BufWriter { buf: &mut buf, idx: 0 };
        
        if core::fmt::write(&mut writer, fmt).is_ok() && writer.idx > 0 {
            self.write_all(&buf[..writer.idx])?;
            return Ok(());
        }
        
        // Si le buffer est trop petit, utiliser un Vec
        let mut output = Vec::new();
        core::fmt::write(&mut output, fmt)
            .map_err(|_| super::IoError::Other)?;
        self.write_all(&output)
    }
}

impl Write for alloc::vec::Vec<u8> {
    fn write(&mut self, buf: &[u8]) -> SizeResult {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }
    
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_all() {
        let mut buf = Vec::new();
        buf.write_all(b"Hello, ").unwrap();
        buf.write_all(b"Exo-OS!").unwrap();
        assert_eq!(buf, b"Hello, Exo-OS!");
    }

    #[test]
    fn test_write_fmt() {
        let mut buf = Vec::new();
        buf.write_fmt(format_args!("Hello, {}! Value: {}", "Exo-OS", 42)).unwrap();
        assert_eq!(buf, b"Hello, Exo-OS! Value: 42");
    }
}