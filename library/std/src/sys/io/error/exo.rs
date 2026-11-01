use crate::io;

pub fn errno() -> i32 {
    0
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
