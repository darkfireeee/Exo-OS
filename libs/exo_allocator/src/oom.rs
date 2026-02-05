//! Out-of-memory handler

use crate::AllocError;

/// OOM handler result
pub type OomResult = Result<(), AllocError>;

/// Install custom OOM handler
pub fn set_oom_handler(_handler: fn() -> OomResult) {
    // Implementation in implementation phase
}

/// Default OOM handler (aborts)
pub fn default_oom_handler() -> OomResult {
    Err(AllocError::OutOfMemory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_oom() {
        assert!(default_oom_handler().is_err());
    }
}
