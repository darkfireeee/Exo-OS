//! Service discovery and registry for Exo-OS

#![no_std]

pub mod registry;
pub mod discovery;
pub mod health;
pub mod storage;

/// Service endpoint
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub path: &'static str,
}

/// Registry errors
#[derive(Debug, Clone, Copy)]
pub enum RegistryError {
    /// Service not found
    NotFound,
    /// Already registered
    AlreadyExists,
    /// Storage error
    StorageError,
    /// Health check failed
    HealthCheckFailed,
}

impl core::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RegistryError::NotFound => write!(f, "Service not found"),
            RegistryError::AlreadyExists => write!(f, "Service already registered"),
            RegistryError::StorageError => write!(f, "Storage error"),
            RegistryError::HealthCheckFailed => write!(f, "Health check failed"),
        }
    }
}

pub type Result<T> = core::result::Result<T, RegistryError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint() {
        let ep = Endpoint { path: "/tmp/test.sock" };
        assert_eq!(ep.path, "/tmp/test.sock");
    }
}
