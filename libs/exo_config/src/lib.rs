//! Configuration management for Exo-OS
//!
//! Hierarchical configuration loading with hot-reload support.

#![no_std]

pub mod loader;
pub mod validator;
pub mod merger;

#[cfg(feature = "hot_reload")]
pub mod watcher;

pub mod migrate;

/// Configuration errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// Parse error
    ParseError,
    /// File not found
    NotFound,
    /// Invalid schema
    InvalidSchema,
    /// Validation failed
    ValidationFailed,
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::ParseError => write!(f, "Parse error"),
            ConfigError::NotFound => write!(f, "Config file not found"),
            ConfigError::InvalidSchema => write!(f, "Invalid schema"),
            ConfigError::ValidationFailed => write!(f, "Validation failed"),
        }
    }
}

pub type Result<T> = core::result::Result<T, ConfigError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        assert_eq!(ConfigError::NotFound.to_string(), "Config file not found");
    }
}
