//! Cursor pour I/O en mémoire

use crate::error::IoErrorKind as IoError;
use super::traits::{Read, Write, Seek, SeekFrom};

/// Cursor pour lecture/écriture en mémoire
pub struct Cursor<T> {
    inner: T,
    pos: u64,
}

impl<T> Cursor<T> {
    /// Crée un nouveau Cursor
    pub const fn new(inner: T) -> Self {
        Self { inner, pos: 0 }
    }

    /// Retourne la position courante
    pub const fn position(&self) -> u64 {
        self.pos
    }

    /// Définit la position
    pub fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }

    /// Retourne une référence au contenu
    pub const fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Retourne une référence mutable au contenu
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consomme le Cursor et retourne le contenu
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: AsRef<[u8]>> Read for Cursor<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        let data = self.inner.as_ref();
        let pos = self.pos as usize;

        if pos >= data.len() {
            return Ok(0);
        }

        let remaining = &data[pos..];
        let to_read = core::cmp::min(buf.len(), remaining.len());
        buf[..to_read].copy_from_slice(&remaining[..to_read]);
        self.pos += to_read as u64;
        Ok(to_read)
    }
}

impl<T: AsMut<[u8]>> Write for Cursor<T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        let data = self.inner.as_mut();
        let pos = self.pos as usize;

        if pos >= data.len() {
            return Ok(0);
        }

        let remaining = &mut data[pos..];
        let to_write = core::cmp::min(buf.len(), remaining.len());
        remaining[..to_write].copy_from_slice(&buf[..to_write]);
        self.pos += to_write as u64;
        Ok(to_write)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

impl<T: AsRef<[u8]>> Seek for Cursor<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, IoError> {
        let len = self.inner.as_ref().len() as u64;

        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => len as i64 + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };

        if new_pos < 0 {
            return Err(IoError::InvalidInput);
        }

        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

impl<T: Clone> Clone for Cursor<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            pos: self.pos,
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Cursor<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cursor")
            .field("inner", &self.inner)
            .field("pos", &self.pos)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_read_write() {
        let mut buf = [0u8; 10];
        let mut cursor = Cursor::new(&mut buf[..]);

        cursor.write(b"hello").unwrap();
        assert_eq!(cursor.position(), 5);

        cursor.set_position(0);
        let mut read_buf = [0u8; 5];
        cursor.read(&mut read_buf).unwrap();
        assert_eq!(&read_buf, b"hello");
    }

    #[test]
    fn test_cursor_seek() {
        let data = b"hello world";
        let mut cursor = Cursor::new(&data[..]);

        cursor.seek(SeekFrom::Start(6)).unwrap();
        assert_eq!(cursor.position(), 6);

        cursor.seek(SeekFrom::Current(-2)).unwrap();
        assert_eq!(cursor.position(), 4);

        cursor.seek(SeekFrom::End(-5)).unwrap();
        assert_eq!(cursor.position(), 6);
    }
}
