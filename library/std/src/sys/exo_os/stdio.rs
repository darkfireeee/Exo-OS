//! `std::sys::exo_os::stdio` — Standard I/O (`Stdin`, `Stdout`, `Stderr`) and `io` traits.
//!
//! Registers a flush hook via [`exo_libc::register_stdio_flush_hook`] so `fork()`
//! flushes user-space stdout buffers before cloning the address space (`PROC-04`).

use crate::alloc_crate::string::String;
use crate::alloc_crate::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use exo_libc::{ExoOs, Pal};

/// I/O error kinds matching `std::io::ErrorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    Interrupted,
    UnexpectedEof,
    OutOfMemory,
    Other,
}

/// Standard I/O error type (`std::io::Error`).
#[derive(Debug, Clone)]
pub struct Error {
    kind: ErrorKind,
    os_code: Option<i32>,
}

impl Error {
    #[inline]
    pub const fn new(kind: ErrorKind, _msg: &str) -> Self {
        Self { kind, os_code: None }
    }

    #[inline]
    pub const fn from_raw_os_error(code: i32) -> Self {
        let pos = if code < 0 { -code } else { code };
        let kind = match pos {
            2 => ErrorKind::NotFound,
            4 => ErrorKind::Interrupted,
            11 => ErrorKind::WouldBlock,
            12 => ErrorKind::OutOfMemory,
            13 | 1 => ErrorKind::PermissionDenied,
            17 => ErrorKind::AlreadyExists,
            22 => ErrorKind::InvalidInput,
            32 => ErrorKind::BrokenPipe,
            98 => ErrorKind::AddrInUse,
            99 => ErrorKind::AddrNotAvailable,
            104 => ErrorKind::ConnectionReset,
            107 => ErrorKind::NotConnected,
            110 => ErrorKind::TimedOut,
            111 => ErrorKind::ConnectionRefused,
            _ => ErrorKind::Other,
        };
        Self {
            kind,
            os_code: Some(pos),
        }
    }

    #[inline]
    pub fn last_os_error() -> Self {
        Self::from_raw_os_error(exo_libc::errno())
    }

    #[inline]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[inline]
    pub const fn raw_os_error(&self) -> Option<i32> {
        self.os_code
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.os_code {
            write!(f, "os error {} ({:?})", code, self.kind)
        } else {
            write!(f, "io error ({:?})", self.kind)
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// `std::io::Read` trait.
pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let mut total = 0usize;
        let mut chunk = [0u8; 512];
        loop {
            match self.read(&mut chunk) {
                Ok(0) => return Ok(total),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    total += n;
                }
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let mut raw = Vec::new();
        let n = self.read_to_end(&mut raw)?;
        match core::str::from_utf8(&raw) {
            Ok(s) => {
                buf.push_str(s);
                Ok(n)
            }
            Err(_) => Err(Error::new(ErrorKind::InvalidData, "invalid utf-8")),
        }
    }
}

/// `std::io::Write` trait.
pub trait Write {
    fn write(&mut self, buf: &[u8]) -> Result<usize>;
    fn flush(&mut self) -> Result<()>;

    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => {
                    return Err(Error::new(
                        ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ))
                }
                Ok(n) => buf = &buf[n..],
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
        struct Adapter<'a, T: ?Sized + 'a>(&'a mut T, Option<Error>);
        impl<T: Write + ?Sized> fmt::Write for Adapter<'_, T> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                match self.0.write_all(s.as_bytes()) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        self.1 = Some(e);
                        Err(fmt::Error)
                    }
                }
            }
        }
        let mut output = Adapter(self, None);
        match fmt::write(&mut output, args) {
            Ok(()) => Ok(()),
            Err(_) => Err(output
                .1
                .unwrap_or(Error::new(ErrorKind::Other, "formatter error"))),
        }
    }
}

static STDIO_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

fn flush_stdout_hook() {
    let _ = ExoOs::fsync(1);
}

#[inline]
fn ensure_flush_hook() {
    if !STDIO_HOOK_INSTALLED.swap(true, Ordering::Relaxed) {
        exo_libc::register_stdio_flush_hook(flush_stdout_hook);
    }
}

/// Standard input handle (`fd = 0`).
#[derive(Debug, Default)]
pub struct Stdin;

pub struct StdinLock<'a>(pub &'a Stdin);

impl Stdin {
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    #[inline]
    pub fn lock(&self) -> StdinLock<'_> {
        StdinLock(self)
    }

    pub fn read_line(&self, buf: &mut String) -> Result<usize> {
        let mut byte = [0u8; 1];
        let mut bytes = Vec::new();
        loop {
            let n = ExoOs::read(0, &mut byte).map_err(|e| Error::from_raw_os_error(e.errno()))?;
            if n == 0 {
                break;
            }
            bytes.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        match core::str::from_utf8(&bytes) {
            Ok(s) => {
                buf.push_str(s);
                Ok(bytes.len())
            }
            Err(_) => Err(Error::new(ErrorKind::InvalidData, "invalid utf-8 in stdin")),
        }
    }
}

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        ExoOs::read(0, buf).map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

impl Read for StdinLock<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Stdin.read(buf)
    }
}

/// Standard output handle (`fd = 1`).
#[derive(Debug, Default)]
pub struct Stdout;

pub struct StdoutLock<'a>(pub &'a Stdout);

impl Stdout {
    #[inline]
    pub fn new() -> Self {
        ensure_flush_hook();
        Self
    }

    #[inline]
    pub fn lock(&self) -> StdoutLock<'_> {
        ensure_flush_hook();
        StdoutLock(self)
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        ensure_flush_hook();
        ExoOs::write(1, buf).map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for StdoutLock<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Stdout.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Standard error handle (`fd = 2`).
#[derive(Debug, Default)]
pub struct Stderr;

pub struct StderrLock<'a>(pub &'a Stderr);

impl Stderr {
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    #[inline]
    pub fn lock(&self) -> StderrLock<'_> {
        StderrLock(self)
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        ExoOs::write(2, buf).map_err(|e| Error::from_raw_os_error(e.errno()))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for StderrLock<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Stderr.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

#[inline]
pub fn stdin() -> Stdin {
    Stdin::new()
}

#[inline]
pub fn stdout() -> Stdout {
    Stdout::new()
}

#[inline]
pub fn stderr() -> Stderr {
    Stderr::new()
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    let _ = stdout().write_fmt(args);
}

#[doc(hidden)]
pub fn _eprint(args: fmt::Arguments<'_>) {
    let _ = stderr().write_fmt(args);
}
