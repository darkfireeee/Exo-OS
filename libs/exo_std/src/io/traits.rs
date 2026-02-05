//! Traits I/O fondamentaux

use crate::error::IoError;

/// Position de seek
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    Start(u64),
    End(i64),
    Current(i64),
}

/// Trait pour la lecture
pub trait Read {
    /// Lit des données dans le buffer
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError>;

    /// Lit exactement `buf.len()` bytes
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), IoError> {
        let mut total = 0;
        while total < buf.len() {
            match self.read(&mut buf[total..]) {
                Ok(0) => return Err(IoError::UnexpectedEof),
                Ok(n) => total += n,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Lit jusqu'à la fin dans un buffer
    fn read_to_end(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        let mut total = 0;
        loop {
            match self.read(&mut buf[total..]) {
                Ok(0) => return Ok(total),
                Ok(n) => total += n,
                Err(e) => return Err(e),
            }
        }
    }

    /// Retourne un itérateur sur les bytes
    fn bytes(self) -> Bytes<Self>
    where
        Self: Sized,
    {
        Bytes { inner: self }
    }

    /// Chain avec un autre reader
    fn chain<R: Read>(self, next: R) -> Chain<Self, R>
    where
        Self: Sized,
    {
        Chain {
            first: self,
            second: next,
            first_done: false,
        }
    }

    /// Take exactement n bytes
    fn take(self, limit: u64) -> Take<Self>
    where
        Self: Sized,
    {
        Take {
            inner: self,
            limit,
        }
    }
}

/// Trait pour l'écriture
pub trait Write {
    /// Écrit des données depuis le buffer
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError>;

    /// Écrit tout le buffer
    fn write_all(&mut self, buf: &[u8]) -> Result<(), IoError> {
        let mut total = 0;
        while total < buf.len() {
            match self.write(&buf[total..]) {
                Ok(0) => return Err(IoError::WriteZero),
                Ok(n) => total += n,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Flush les buffers
    fn flush(&mut self) -> Result<(), IoError>;

    /// Écrit un string formaté
    fn write_fmt(&mut self, fmt: core::fmt::Arguments<'_>) -> Result<(), IoError> {
        struct Adapter<'a, T: ?Sized>(&'a mut T);

        impl<T: Write + ?Sized> core::fmt::Write for Adapter<'_, T> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                self.0.write_all(s.as_bytes()).map_err(|_| core::fmt::Error)
            }
        }

        let mut adapter = Adapter(self);
        core::fmt::write(&mut adapter, fmt).map_err(|_| IoError::InvalidData)
    }
}

/// Trait pour seek (déplacement de position)
pub trait Seek {
    /// Se déplace à une position
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, IoError>;

    /// Retourne la position courante
    fn stream_position(&mut self) -> Result<u64, IoError> {
        self.seek(SeekFrom::Current(0))
    }

    /// Revient au début
    fn rewind(&mut self) -> Result<(), IoError> {
        self.seek(SeekFrom::Start(0))?;
        Ok(())
    }
}

/// Itérateur sur les bytes
pub struct Bytes<R> {
    inner: R,
}

impl<R: Read> Iterator for Bytes<R> {
    type Item = Result<u8, IoError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut byte = [0];
        match self.inner.read(&mut byte) {
            Ok(0) => None,
            Ok(_) => Some(Ok(byte[0])),
            Err(e) => Some(Err(e)),
        }
    }
}

/// Chain de deux readers
pub struct Chain<T, U> {
    first: T,
    second: U,
    first_done: bool,
}

impl<T: Read, U: Read> Read for Chain<T, U> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        if !self.first_done {
            match self.first.read(buf) {
                Ok(0) => {
                    self.first_done = true;
                    self.second.read(buf)
                }
                Ok(n) => Ok(n),
                Err(e) => Err(e),
            }
        } else {
            self.second.read(buf)
        }
    }
}

/// Reader limité à n bytes
pub struct Take<T> {
    inner: T,
    limit: u64,
}

impl<T: Read> Read for Take<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        if self.limit == 0 {
            return Ok(0);
        }

        let max = core::cmp::min(buf.len() as u64, self.limit) as usize;
        let n = self.inner.read(&mut buf[..max])?;
        self.limit -= n as u64;
        Ok(n)
    }
}

/// Empty reader (ne lit rien)
pub struct Empty;

impl Read for Empty {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize, IoError> {
        Ok(0)
    }
}

/// Sink writer (écrit dans le vide)
pub struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

/// Retourne un reader vide
pub const fn empty() -> Empty {
    Empty
}

/// Retourne un writer sink
pub const fn sink() -> Sink {
    Sink
}
