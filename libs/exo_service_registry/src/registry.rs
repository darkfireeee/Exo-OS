//! Service registry

use crate::{Endpoint, Result, RegistryError};

/// Service registry
pub struct Registry;

impl Registry {
    /// Create new registry
    pub fn new() -> Self {
        Self
    }

    /// Register service
    pub fn register(&mut self, _name: &str, _endpoint: &str) -> Result<()> {
        Ok(())
    }

    /// Find service
    pub fn find(&self, _name: &str) -> Result<Endpoint> {
        Err(RegistryError::NotFound)
    }

    /// Unregister service
    pub fn unregister(&mut self, _name: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let _registry = Registry::new();
    }
}
