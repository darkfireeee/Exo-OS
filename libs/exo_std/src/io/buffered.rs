//! Readers et writers bufferisés

use crate::error::IoError;
use super::traits::{Read, Write};

const DEFAULT_BUF_SIZE: usize = 8192;

/// Reader bufferisé
pub struct BufReader<R> {
    inner: R,
    buf: [u8; DEFAULT_BUF_SIZE],
    pos: usize,
    cap: usize,
}

impl<R: Read> BufReader<R> {
    /// Crée un nouveau BufReader
    pub const fn new(inner: R) -> Self {
        Self {
            inner,
            buf: [0; DEFAULT_BUF_SIZE],
            pos: 0,
            cap: 0,
        }
    }

    /// Crée un BufReader avec capacité spécifiée (pour compatibilité API)
    pub const fn with_capacity(_capacity: usize, inner: R) -> Self {
        // Note: Ignore capacity pour l'instant car on utilise un buffer fixe
        Self::new(inner)
    }

    /// Retourne une référence au reader interne
    pub const fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Retourne une référence mutable au reader interne
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Consomme le BufReader et retourne le reader interne
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Retourne le buffer actuel
    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.pos..self.cap]
    }

    /// Remplit le buffer
    fn fill_buf(&mut self) -> Result<&[u8], IoError> {
        if self.pos >= self.cap {
            self.cap = self.inner.read(&mut self.buf)?;
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..self.cap])
    }

    /// Consomme n bytes du buffer
    fn consume(&mut self, amt: usize) {
        self.pos = core::cmp::min(self.pos + amt, self.cap);
    }

    /// Lit une ligne dans un buffer
    pub fn read_line(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        let mut total = 0;

        loop {
            let available = self.fill_buf()?;
            if available.is_empty() {
                break;
            }

            // Cherche un newline
            let mut found_newline = false;
            let mut bytes_to_copy = 0;

            for (i, &byte) in available.iter().enumerate() {
                if total + i >= buf.len() {
                    break;
                }
                buf[total + i] = byte;
                bytes_to_copy = i + 1;
                if byte == b'\n' {
                    found_newline = true;
                    break;
                }
            }

            total += bytes_to_copy;
            self.consume(bytes_to_copy);

            if found_newline || total >= buf.len() {
                break;
            }
        }

        Ok(total)
    }
}

impl<R: Read> Read for BufReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        // Si le buffer demandé est plus grand que notre buffer interne,
        // lire directement
        if buf.len() >= DEFAULT_BUF_SIZE {
            return self.inner.read(buf);
        }

        let available = self.fill_buf()?;
        let to_read = core::cmp::min(buf.len(), available.len());
        buf[..to_read].copy_from_slice(&available[..to_read]);
        self.consume(to_read);
        Ok(to_read)
    }
}

/// Writer bufferisé
pub struct BufWriter<W> {
    inner: W,
    buf: [u8; DEFAULT_BUF_SIZE],
    pos: usize,
}

impl<W: Write> BufWriter<W> {
    /// Crée un nouveau BufWriter
    pub const fn new(inner: W) -> Self {
        Self {
            inner,
            buf: [0; DEFAULT_BUF_SIZE],
            pos: 0,
        }
    }

    /// Crée un BufWriter avec capacité spécifiée (pour compatibilité API)
    pub const fn with_capacity(_capacity: usize, inner: W) -> Self {
        Self::new(inner)
    }

    /// Retourne une référence au writer interne
    pub const fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Retourne une référence mutable au writer interne
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consomme le BufWriter et retourne le writer interne
    pub fn into_inner(mut self) -> Result<W, IoError> {
        self.flush_buf()?;
        Ok(self.inner)
    }

    /// Retourne le buffer actuel
    pub fn buffer(&self) -> &[u8] {
        &self.buf[..self.pos]
    }

    /// Flush le buffer interne
    fn flush_buf(&mut self) -> Result<(), IoError> {
        if self.pos > 0 {
            self.inner.write_all(&self.buf[..self.pos])?;
            self.pos = 0;
        }
        Ok(())
    }
}

impl<W: Write> Write for BufWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        // Si le buffer est plus grand que notre capacité, flush et écrire directement
        if buf.len() >= DEFAULT_BUF_SIZE {
            self.flush_buf()?;
            return self.inner.write(buf);
        }

        // Si ça ne rentre pas dans le buffer, flush d'abord
        if self.pos + buf.len() > DEFAULT_BUF_SIZE {
            self.flush_buf()?;
        }

        // Copier dans le buffer
        let to_write = core::cmp::min(buf.len(), DEFAULT_BUF_SIZE - self.pos);
        self.buf[self.pos..self.pos + to_write].copy_from_slice(&buf[..to_write]);
        self.pos += to_write;

        Ok(to_write)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.flush_buf()?;
        self.inner.flush()
    }
}

impl<W> Drop for BufWriter<W> {
    fn drop(&mut self) {
        // Best effort flush - ignore errors in drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Cursor;

    #[test]
    fn test_buf_reader() {
        let data = b"hello\nworld\n";
        let reader = Cursor::new(&data[..]);
        let mut buf_reader = BufReader::new(reader);

        let mut line = [0u8; 10];
        let n = buf_reader.read_line(&mut line).unwrap();
        assert_eq!(&line[..n], b"hello\n");

        let n = buf_reader.read_line(&mut line).unwrap();
        assert_eq!(&line[..n], b"world\n");
    }

    #[test]
    fn test_buf_writer() {
        let mut buf = [0u8; 100];
        let cursor = Cursor::new(&mut buf[..]);
        let mut buf_writer = BufWriter::new(cursor);

        buf_writer.write(b"hello").unwrap();
        buf_writer.write(b" ").unwrap();
        buf_writer.write(b"world").unwrap();
        buf_writer.flush().unwrap();

        let cursor = buf_writer.into_inner().unwrap();
        let written = &cursor.get_ref()[..11];
        assert_eq!(written, b"hello world");
    }
}
