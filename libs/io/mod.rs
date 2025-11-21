// libs/exo_std/src/io/mod.rs
pub mod read;
pub mod write;
pub mod seek;

pub use read::Read;
pub use write::Write;
pub use seek::Seek;

/// Erreur d'E/S
#[derive(Debug)]
pub enum IoError {
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
    Other,
    UnexpectedEof,
    Custom(String),
}

impl From<exo_types::ExoError> for IoError {
    fn from(err: exo_types::ExoError) -> Self {
        match err.code() {
            exo_types::ErrorCode::NotFound => IoError::NotFound,
            exo_types::ErrorCode::PermissionDenied => IoError::PermissionDenied,
            exo_types::ErrorCode::IoError => IoError::Other,
            _ => IoError::Other,
        }
    }
}

/// Type de résultat pour les opérations d'E/S
pub type Result<T> = core::result::Result<T, IoError>;

/// Type de résultat pour les opérations d'E/S avec taille
pub type SizeResult = core::result::Result<usize, IoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_error_conversion() {
        let err = exo_types::ExoError::new(exo_types::ErrorCode::NotFound);
        let io_err: IoError = err.into();
        assert_eq!(io_err, IoError::NotFound);
    }
}