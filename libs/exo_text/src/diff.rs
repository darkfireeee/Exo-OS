//! Unified diff generator

use crate::{Result, TextError};

/// Generate unified diff
pub fn unified(_old: &str, _new: &str) -> Result<()> {
    Err(TextError::ParseError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_stub() {
        assert!(unified("a", "b").is_err());
    }
}
