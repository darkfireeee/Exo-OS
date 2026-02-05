//! Configuration loader

use crate::{Result, ConfigError};

/// Configuration loader
pub struct ConfigLoader;

impl ConfigLoader {
    /// Create new loader
    pub fn new() -> Self {
        Self
    }

    /// Add config file
    pub fn add_file(&mut self, _path: &str) -> &mut Self {
        self
    }

    /// Load configuration
    pub fn load(&self) -> Result<()> {
        Err(ConfigError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_creation() {
        let _loader = ConfigLoader::new();
    }
}
