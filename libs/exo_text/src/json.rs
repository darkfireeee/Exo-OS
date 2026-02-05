//! RFC 8259 JSON parser (no dependencies)

use crate::{Result, TextError};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

pub struct JsonParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> JsonParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }
    
    pub fn parse(&mut self) -> Result<JsonValue> {
        self.skip_whitespace();
        self.parse_value()
    }
    
    fn parse_value(&mut self) -> Result<JsonValue> {
        self.skip_whitespace();
        
        match self.peek() {
            Some('n') => self.parse_null(),
            Some('t') | Some('f') => self.parse_bool(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('[') => self.parse_array(),
            Some('{') => self.parse_object(),
            Some(c) if c.is_ascii_digit() || c == '-' => self.parse_number(),
            _ => Err(TextError::UnexpectedToken),
        }
    }
    
    fn parse_null(&mut self) -> Result<JsonValue> {
        if self.consume_str("null") {
            Ok(JsonValue::Null)
        } else {
            Err(TextError::ParseError)
        }
    }
    
    fn parse_bool(&mut self) -> Result<JsonValue> {
        if self.consume_str("true") {
            Ok(JsonValue::Bool(true))
        } else if self.consume_str("false") {
            Ok(JsonValue::Bool(false))
        } else {
            Err(TextError::ParseError)
        }
    }
    
    fn parse_number(&mut self) -> Result<JsonValue> {
        let start = self.pos;
        
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        
        let num_str = &self.input[start..self.pos];
        num_str.parse::<f64>()
            .map(JsonValue::Number)
            .map_err(|_| TextError::ParseError)
    }
    
    fn parse_string(&mut self) -> Result<String> {
        if self.consume('"') {
            let mut result = String::new();
            
            while let Some(c) = self.peek() {
                if c == '"' {
                    self.pos += 1;
                    return Ok(result);
                } else if c == '\\' {
                    self.pos += 1;
                    match self.peek() {
                        Some('"') => { result.push('"'); self.pos += 1; }
                        Some('\\') => { result.push('\\'); self.pos += 1; }
                        Some('n') => { result.push('\n'); self.pos += 1; }
                        Some('r') => { result.push('\r'); self.pos += 1; }
                        Some('t') => { result.push('\t'); self.pos += 1; }
                        _ => return Err(TextError::ParseError),
                    }
                } else {
                    result.push(c);
                    self.pos += 1;
                }
            }
            Err(TextError::UnexpectedEof)
        } else {
            Err(TextError::ParseError)
        }
    }
    
    fn parse_array(&mut self) -> Result<JsonValue> {
        if !self.consume('[') {
            return Err(TextError::ParseError);
        }
        
        let mut elements = Vec::new();
        self.skip_whitespace();
        
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(JsonValue::Array(elements));
        }
        
        loop {
            elements.push(self.parse_value()?);
            self.skip_whitespace();
            
            if self.consume(',') {
                continue;
            } else if self.consume(']') {
                return Ok(JsonValue::Array(elements));
            } else {
                return Err(TextError::ParseError);
            }
        }
    }
    
    fn parse_object(&mut self) -> Result<JsonValue> {
        if !self.consume('{') {
            return Err(TextError::ParseError);
        }
        
        let mut pairs = Vec::new();
        self.skip_whitespace();
        
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(JsonValue::Object(pairs));
        }
        
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            
            if !self.consume(':') {
                return Err(TextError::ParseError);
            }
            
            let value = self.parse_value()?;
            pairs.push((key, value));
            
            self.skip_whitespace();
            if self.consume(',') {
                continue;
            } else if self.consume('}') {
                return Ok(JsonValue::Object(pairs));
            } else {
                return Err(TextError::ParseError);
            }
        }
    }
    
    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }
    
    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }
    
    fn consume_str(&mut self, s: &str) -> bool {
        if self.input[self.pos..].starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
    
    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }
}

pub fn parse(input: &str) -> Result<JsonValue> {
    JsonParser::new(input).parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_null() {
        assert_eq!(parse("null").unwrap(), JsonValue::Null);
    }
    
    #[test]
    fn test_parse_bool() {
        assert_eq!(parse("true").unwrap(), JsonValue::Bool(true));
        assert_eq!(parse("false").unwrap(), JsonValue::Bool(false));
    }
    
    #[test]
    fn test_parse_number() {
        assert_eq!(parse("42").unwrap(), JsonValue::Number(42.0));
        assert_eq!(parse("-3.14").unwrap(), JsonValue::Number(-3.14));
    }
    
    #[test]
    fn test_parse_string() {
        assert_eq!(parse(r#""hello""#).unwrap(), JsonValue::String("hello".into()));
    }
    
    #[test]
    fn test_parse_array() {
        let result = parse("[1, 2, 3]").unwrap();
        match result {
            JsonValue::Array(arr) => assert_eq!(arr.len(), 3),
            _ => panic!("Expected array"),
        }
    }
    
    #[test]
    fn test_parse_object() {
        let result = parse(r#"{"key": "value"}"#).unwrap();
        match result {
            JsonValue::Object(obj) => assert_eq!(obj.len(), 1),
            _ => panic!("Expected object"),
        }
    }
}
