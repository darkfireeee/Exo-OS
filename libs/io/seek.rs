// libs/exo_std/src/io/seek.rs
use super::Result;

/// Positions de recherche dans un flux
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SeekFrom {
    /// Début du flux
    Start(u64),
    /// Fin du flux
    End(i64),
    /// Position actuelle
    Current(i64),
}

/// Trait pour les types qui supportent la recherche de position
pub trait Seek {
    /// Change la position courante dans le flux
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;
}

impl Seek for alloc::vec::Vec<u8> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(pos) => pos as usize,
            SeekFrom::End(pos) => {
                let len = self.len() as i64;
                let pos = len + pos;
                if pos < 0 {
                    return Err(super::IoError::InvalidInput);
                }
                pos as usize
            }
            SeekFrom::Current(pos) => {
                let current = self.len() as i64;
                let pos = current + pos;
                if pos < 0 {
                    return Err(super::IoError::InvalidInput);
                }
                pos as usize
            }
        };
        
        if new_pos > self.len() {
            self.resize(new_pos, 0);
        }
        
        Ok(new_pos as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seek_vec() {
        let mut buf = Vec::from(b"Hello, world!" as &[u8]);
        
        // Aller au début
        assert_eq!(buf.seek(SeekFrom::Start(0)).unwrap(), 0);
        
        // Aller à la fin
        assert_eq!(buf.seek(SeekFrom::End(0)).unwrap(), 13);
        
        // Aller à une position relative
        assert_eq!(buf.seek(SeekFrom::Current(-5)).unwrap(), 8);
        
        // Étendre le buffer
        assert_eq!(buf.seek(SeekFrom::Start(20)).unwrap(), 20);
        assert_eq!(buf.len(), 20);
    }
}