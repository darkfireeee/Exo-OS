//! Markdown parsing and rendering

use crate::{Result, TextError};

/// Render Markdown to HTML
pub fn to_html(_input: &str) -> Result<()> {
    Err(TextError::ParseError)
}

/// Render Markdown to terminal (ANSI colors)
pub fn to_terminal(_input: &str) -> Result<()> {
    Err(TextError::ParseError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_stub() {
        assert!(to_html("# Test").is_err());
    }
}
