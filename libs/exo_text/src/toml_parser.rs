//! TOML v1.0 parser (simplified)

use crate::{Result, TextError};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum TomlValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<TomlValue>),
    Table(Vec<(String, TomlValue)>),
}

pub struct TomlParser<'a> {
    #[allow(dead_code)]
    input: &'a str,
    lines: Vec<&'a str>,
    pos: usize,
}

impl<'a> TomlParser<'a> {
    pub fn new(input: &'a str) -> Self {
        let lines: Vec<&str> = input.lines().collect();
        Self {
            input,
            lines,
            pos: 0,
        }
    }

    pub fn parse(&mut self) -> Result<TomlValue> {
        let mut table = Vec::new();

        while self.pos < self.lines.len() {
            let line = self.lines[self.pos].trim();

            if line.is_empty() || line.starts_with('#') {
                self.pos += 1;
                continue;
            }

            if let Some(_section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                // Skip section headers for now
                self.pos += 1;
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_string();
                let value = self.parse_value(value.trim())?;
                table.push((key, value));
            }

            self.pos += 1;
        }

        Ok(TomlValue::Table(table))
    }

    fn parse_value(&self, s: &str) -> Result<TomlValue> {
        let s = s.trim();

        // String
        if s.starts_with('"') && s.ends_with('"') {
            return Ok(TomlValue::String(s[1..s.len() - 1].to_string()));
        }

        // Boolean
        if s == "true" {
            return Ok(TomlValue::Boolean(true));
        }
        if s == "false" {
            return Ok(TomlValue::Boolean(false));
        }

        // Integer
        if let Ok(i) = s.parse::<i64>() {
            return Ok(TomlValue::Integer(i));
        }

        // Float
        if let Ok(f) = s.parse::<f64>() {
            return Ok(TomlValue::Float(f));
        }

        Err(TextError::ParseError)
    }
}

pub fn from_str(input: &str) -> Result<TomlValue> {
    TomlParser::new(input).parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_toml() {
        let input = r#"
key1 = "value1"
key2 = 42
key3 = true
        "#;

        let result = from_str(input).unwrap();
        match result {
            TomlValue::Table(t) => {
                assert_eq!(t.len(), 3);
            }
            _ => panic!("Expected table"),
        }
    }
}
