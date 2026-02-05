//! Text processing utilities for Exo-OS
//!
//! Production parsers (no dependencies):
//! - JSON: RFC 8259 streaming parser
//! - TOML: v1.0 configuration parser
//! - Markdown: CommonMark subset
//! - Diff: Unified format

#![no_std]

extern crate alloc;

pub mod json;
pub mod toml_parser;
pub mod markdown;
pub mod diff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    ParseError,
    InvalidUtf8,
    InvalidFormat,
    BufferTooSmall,
    UnexpectedToken,
    UnexpectedEof,
}

pub type Result<T> = core::result::Result<T, TextError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        assert_eq!(TextError::ParseError.to_string(), "Parse error");
    }
}
