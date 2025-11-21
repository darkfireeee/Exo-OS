// libs/exo_std/src/io/read.rs
use super::{Result, SizeResult};

/// Trait pour les types qui peuvent lire des octets
pub trait Read {
    /// Lit des octets dans le buffer fourni
    fn read(&mut self, buf: &mut [u8]) -> SizeResult;
    
    /// Lit exactement le nombre d'octets demandés
    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.read(buf) {
                Ok(0) => return Err(super::IoError::UnexpectedEof),
                Ok(n) => {
                    let tmp = buf;
                    buf = &mut tmp[n..];
                }
                Err(ref e) if e.kind() == super::IoError::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    
    /// Lit jusqu'à ce que le buffer soit plein ou EOF
    fn read_to_end(&mut self, buf: &mut alloc::vec::Vec<u8>) -> SizeResult {
        let start_len = buf.len();
        let mut len = start_len;
        let mut new_write_size = 64;
        
        loop {
            if len == buf.len() {
                // Agrandir le buffer
                buf.resize(len + new_write_size, 0);
                new_write_size *= 2;
            }
            
            match self.read(&mut buf[len..]) {
                Ok(0) => {
                    buf.truncate(len);
                    return Ok(len - start_len);
                }
                Ok(n) => len += n,
                Err(ref e) if e.kind() == super::IoError::Interrupted => {}
                Err(e) => {
                    buf.truncate(len);
                    return Err(e);
                }
            }
        }
    }
}

impl Read for &[u8] {
    fn read(&mut self, buf: &mut [u8]) -> SizeResult {
        let amt = core::cmp::min(buf.len(), self.len());
        if amt == 0 {
            return Ok(0);
        }
        
        buf[..amt].copy_from_slice(&self[..amt]);
        *self = &self[amt..];
        Ok(amt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_exact() {
        let mut data = b"Hello, world!" as &[u8];
        let mut buf = [0u8; 13];
        
        data.read_exact(&mut buf).unwrap();
        assert_eq!(buf, *b"Hello, world!");
    }

    #[test]
    fn test_read_to_end() {
        let mut data = b"Hello, Exo-OS!" as &[u8];
        let mut buf = alloc::vec::Vec::new();
        
        let bytes_read = data.read_to_end(&mut buf).unwrap();
        assert_eq!(bytes_read, 15);
        assert_eq!(buf, b"Hello, Exo-OS!");
    }
}