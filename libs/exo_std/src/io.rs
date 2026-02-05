// libs/exo_std/src/io.rs
//! Opérations d'I/O robustes et type-safe
//!
//! Ce module fournit des traits et implémentations pour les opérations
//! d'entrée/sortie, avec gestion d'erreurs complète et support du buffering.

use core::fmt;
use crate::error::{ExoStdError, IoErrorKind};
use crate::syscall::io as sys_io;

pub type Result<T> = core::result::Result<T, ExoStdError>;

/// Trait pour la lecture de données
///
/// Fournit des méthodes pour lire des bytes depuis une source.
pub trait Read {
    /// Lit des données dans le buffer
    ///
    /// Retourne le nombre de bytes lus, 0 signifie EOF.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    
    /// Lit jusqu'à remplir complètement le buffer
    ///
    /// Retourne une erreur si EOF avant que le buffer soit plein.
    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.read(buf) {
                Ok(0) => return Err(IoErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    buf = &mut buf[n..];
                }
                Err(e) if matches!(e, ExoStdError::Io(IoErrorKind::Interrupted)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    
    /// Lit tous les bytes jusqu'à EOF
    fn read_to_end(&mut self, buf: &mut alloc::vec::Vec<u8>) -> Result<usize> {
        let start_len = buf.len();
        let mut temp = [0u8; 1024];
        
        loop {
            match self.read(&mut temp) {
                Ok(0) => return Ok(buf.len() - start_len),
                Ok(n) => buf.extend_from_slice(&temp[..n]),
                Err(e) if matches!(e, ExoStdError::Io(IoErrorKind::Interrupted)) => {}
                Err(e) => return Err(e),
            }
        }
    }
    
    /// Lit dans une chaîne
    fn read_to_string(&mut self, buf: &mut alloc::string::String) -> Result<usize> {
        let mut bytes = alloc::vec::Vec::new();
        let len = self.read_to_end(&mut bytes)?;
        
        match alloc::string::String::from_utf8(bytes) {
            Ok(s) => {
                buf.push_str(&s);
                Ok(len)
            }
            Err(_) => Err(IoErrorKind::InvalidData.into()),
        }
    }
    
    /// Crée un itérateur sur les bytes
    fn bytes(self) -> Bytes<Self>
    where
        Self: Sized,
    {
        Bytes { inner: self }
    }
}

/// Trait pour l'écriture de données
pub trait Write {
    /// Écrit des données depuis le buffer
    ///
    /// Retourne le nombre de bytes écrits.
    fn write(&mut self, buf: &[u8]) -> Result<usize>;
    
    /// Flush les buffers internes
    fn flush(&mut self) -> Result<()>;
    
    /// Écrit tout le buffer
    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => return Err(IoErrorKind::UnexpectedEof.into()),
                Ok(n) => buf = &buf[n..],
                Err(e) if matches!(e, ExoStdError::Io(IoErrorKind::Interrupted)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    
    /// Écrit une chaîne formatée
    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> Result<()> {
        // Implémentation basique utilisant write_all
        struct Adapter<'a, T: ?Sized> {
            inner: &'a mut T,
            error: Result<()>,
        }
        
        impl<T: Write + ?Sized> fmt::Write for Adapter<'_, T> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                match self.inner.write_all(s.as_bytes()) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        self.error = Err(e);
                        Err(fmt::Error)
                    }
                }
            }
        }
        
        let mut adapter = Adapter {
            inner: self,
            error: Ok(()),
        };
        
        match fmt::write(&mut adapter, fmt) {
            Ok(()) => Ok(()),
            Err(..) => adapter.error,
        }
    }
}

/// Trait pour le positionnement dans un flux
pub trait Seek {
    /// Repositionne le curseur
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;
    
    /// Position actuelle
    fn stream_position(&mut self) -> Result<u64> {
        self.seek(SeekFrom::Current(0))
    }
}

/// Position pour seek
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    /// Depuis le début
    Start(u64),
    /// Depuis la position actuelle
    Current(i64),
    /// Depuis la fin
    End(i64),
}

/// Itérateur sur les bytes d'un Reader
pub struct Bytes<R> {
    inner: R,
}

impl<R: Read> Iterator for Bytes<R> {
    type Item = Result<u8>;
    
    fn next(&mut self) -> Option<Self::Item> {
        let mut byte = 0u8;
        match self.inner.read(core::slice::from_mut(&mut byte)) {
            Ok(0) => None,
            Ok(..) => Some(Ok(byte)),
            Err(e) => Some(Err(e)),
        }
    }
}

/// Entrée standard (stdin)
#[derive(Debug)]
pub struct Stdin;

/// Sortie standard (stdout)
#[derive(Debug)]
pub struct Stdout;

/// Erreur standard (stderr)
#[derive(Debug)]
pub struct Stderr;

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        unsafe {
            sys_io::read(sys_io::STDIN_FD, buf.as_mut_ptr(), buf.len())
                .map_err(|e| e)
        }
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        #[cfg(feature = "test_mode")]
        {
            Ok(buf.len())
        }
        
        #[cfg(not(feature = "test_mode"))]
        unsafe {
            sys_io::write(sys_io::STDOUT_FD, buf.as_ptr(), buf.len())
                .map_err(|e| e)
        }
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        #[cfg(feature = "test_mode")]
        {
            Ok(buf.len())
        }
        
        #[cfg(not(feature = "test_mode"))]
        unsafe {
            sys_io::write(sys_io::STDERR_FD, buf.as_ptr(), buf.len())
                .map_err(|e| e)
        }
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

impl fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

/// Retourne stdin
#[inline]
pub fn stdin() -> Stdin {
    Stdin
}

/// Retourne stdout
#[inline]
pub fn stdout() -> Stdout {
    Stdout
}

/// Retourne stderr
#[inline]
pub fn stderr() -> Stderr {
    Stderr
}

/// Cursor pour lecture/écriture en mémoire
pub struct Cursor<T> {
    inner: T,
    pos: u64,
}

impl<T> Cursor<T> {
    /// Crée un nouveau Cursor
    #[inline]
    pub const fn new(inner: T) -> Self {
        Self { inner, pos: 0 }
    }
    
    /// Position actuelle
    #[inline]
    pub const fn position(&self) -> u64 {
        self.pos
    }
    
    /// Défini la position
    #[inline]
    pub fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }
    
    /// Référence aux données internes
    #[inline]
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
    
    /// Référence mutable aux données
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
    
    /// Consomme et retourne les données
    #[inline]
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: AsRef<[u8]>> Read for Cursor<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let data = self.inner.as_ref();
        let pos = self.pos as usize;
        
        if pos >= data.len() {
            return Ok(0);
        }
        
        let remaining = &data[pos..];
        let to_read = remaining.len().min(buf.len());
        buf[..to_read].copy_from_slice(&remaining[..to_read]);
        
        self.pos += to_read as u64;
        Ok(to_read)
    }
}

impl Write for Cursor<&mut [u8]> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let pos = self.pos as usize;
        let remaining = &mut self.inner[pos..];
        
        let to_write = remaining.len().min(buf.len());
        remaining[..to_write].copy_from_slice(&buf[..to_write]);
        
        self.pos += to_write as u64;
        Ok(to_write)
    }
    
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl<T: AsRef<[u8]>> Seek for Cursor<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let len = self.inner.as_ref().len() as i64;
        
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => len + n,
        };
        
        if new_pos < 0 {
            return Err(IoErrorKind::InvalidInput.into());
        }
        
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cursor_read() {
        let data = b"Hello, world!";
        let mut cursor = Cursor::new(&data[..]);
        
        let mut buf = [0u8; 5];
        assert_eq!(cursor.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf, b"Hello");
        
        assert_eq!(cursor.position(), 5);
    }
    
    #[test]
    fn test_cursor_write() {
        let mut data = [0u8; 10];
        let mut cursor = Cursor::new(&mut data[..]);
        
        assert_eq!(cursor.write(b"test").unwrap(), 4);
        assert_eq!(&data[..4], b"test");
    }
}
