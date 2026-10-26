//! Text processing utilities for Exo-OS
//!
//! Production parsers (no dependencies):
//! - JSON: RFC 8259 streaming parser
//! - TOML: v1.0 configuration parser
//! - Markdown: CommonMark subset
//! - Diff: Unified format

#![no_std]

extern crate alloc;

pub mod diff;
pub mod json;
pub mod markdown;
pub mod toml_parser;

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

impl core::fmt::Display for TextError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ParseError => write!(f, "Parse error"),
            Self::InvalidUtf8 => write!(f, "Invalid UTF-8"),
            Self::InvalidFormat => write!(f, "Invalid format"),
            Self::BufferTooSmall => write!(f, "Buffer too small"),
            Self::UnexpectedToken => write!(f, "Unexpected token"),
            Self::UnexpectedEof => write!(f, "Unexpected EOF"),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn test_error_display() {
        assert_eq!(TextError::ParseError.to_string(), "Parse error");
    }
}
