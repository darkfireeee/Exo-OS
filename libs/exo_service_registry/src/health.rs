//! Health checking

use crate::Result;

/// Health checker
pub struct HealthChecker;

impl HealthChecker {
    /// Create health checker
    pub fn new() -> Self {
        Self
    }

    /// Check all services
    pub fn check_all(&self) -> Result<()> {
        Ok(())
    }

    /// Ping service
    pub fn ping(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}
