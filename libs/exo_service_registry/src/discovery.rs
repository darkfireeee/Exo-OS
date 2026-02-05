//! Service discovery client

use crate::{Endpoint, Result};

/// Discovery client
pub struct Discovery;

impl Discovery {
    /// Create discovery client
    pub fn new() -> Self {
        Self
    }

    /// Find service by name
    pub fn find(&self, _name: &str) -> Result<Endpoint> {
        Err(crate::RegistryError::NotFound)
    }
}
