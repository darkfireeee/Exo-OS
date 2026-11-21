use crate::io;
use crate::sync::atomic::{AtomicI32, Ordering};
use crate::sys::pal::runtime::ExoTcb;

static EARLY_ERRNO: AtomicI32 = AtomicI32::new(0);

#[inline]
pub fn errno_location() -> *mut i32 {
    let ptr = unsafe { ExoTcb::errno_ptr() };
    if ptr.is_null() {
        EARLY_ERRNO.as_ptr()
    } else {
        ptr
    }
}

#[inline]
pub fn errno() -> i32 {
    let ptr = unsafe { ExoTcb::errno_ptr() };
    if ptr.is_null() {
        EARLY_ERRNO.load(Ordering::Relaxed)
    } else {
        unsafe { ptr.read() }
    }
}

#[inline]
pub fn set_errno(e: i32) {
    let ptr = unsafe { ExoTcb::errno_ptr() };
    if ptr.is_null() {
        EARLY_ERRNO.store(e, Ordering::Relaxed);
    } else {
        unsafe { ptr.write(e) };
    }
}

#[inline]
pub fn is_interrupted(code: i32) -> bool {
    code == 4 // EINTR
}

pub fn decode_error_kind(code: i32) -> io::ErrorKind {
    match code {
        1 | 13 => io::ErrorKind::PermissionDenied,
        2 => io::ErrorKind::NotFound,
        4 => io::ErrorKind::Interrupted,
        11 => io::ErrorKind::WouldBlock,
        12 => io::ErrorKind::OutOfMemory,
        16 => io::ErrorKind::ResourceBusy,
        17 => io::ErrorKind::AlreadyExists,
        20 => io::ErrorKind::NotADirectory,
        21 => io::ErrorKind::IsADirectory,
        22 => io::ErrorKind::InvalidInput,
        28 => io::ErrorKind::StorageFull,
        30 => io::ErrorKind::ReadOnlyFilesystem,
        32 => io::ErrorKind::BrokenPipe,
        36 => io::ErrorKind::InvalidFilename,
        38 => io::ErrorKind::Unsupported,
        39 => io::ErrorKind::DirectoryNotEmpty,
        98 => io::ErrorKind::AddrInUse,
        99 => io::ErrorKind::AddrNotAvailable,
        101 => io::ErrorKind::NetworkUnreachable,
        103 => io::ErrorKind::ConnectionAborted,
        104 => io::ErrorKind::ConnectionReset,
        106 => io::ErrorKind::AlreadyExists,
        107 => io::ErrorKind::NotConnected,
        110 => io::ErrorKind::TimedOut,
        111 => io::ErrorKind::ConnectionRefused,
        _ => io::ErrorKind::Uncategorized,
    }
}

pub fn error_string(errno: i32) -> String {
    format!("Exo-OS error (os error {errno})")
}
