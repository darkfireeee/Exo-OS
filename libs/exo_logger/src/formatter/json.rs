//! JSON Lines formatter for log entries

use crate::LogEntry;
use alloc::string::String;
use alloc::format;

pub struct JsonFormatter;

impl JsonFormatter {
    pub fn format(entry: &LogEntry) -> String {
        let mut json = String::from("{");
        
        // Timestamp
        json.push_str(&format!("\"ts\":{},", entry.timestamp));
        
        // Level
        json.push_str(&format!("\"level\":\"{}\",", entry.level.as_str()));
        
        // Target
        json.push_str("\"target\":\"");
        escape_json_string(&entry.target, &mut json);
        json.push_str("\",");
        
        // Message
        json.push_str("\"msg\":\"");
        escape_json_string(&entry.message, &mut json);
        json.push('"');
        
        // Span ID
        if let Some(span_id) = entry.span_id {
            json.push_str(&format!(",\"span\":{}", span_id.as_u64()));
        }
        
        // Custom fields
        if !entry.fields.is_empty() {
            json.push_str(",\"fields\":");
            json.push('{');
            for (i, (key, value)) in entry.fields.iter().enumerate() {
                if i > 0 {
                    json.push(',');
                }
                json.push('"');
                escape_json_string(key, &mut json);
                json.push_str("\":\"");
                escape_json_string(value, &mut json);
                json.push('"');
            }
            json.push('}');
        }
        
        json.push('}');
        json
    }
}

fn escape_json_string(s: &str, output: &mut String) {
    for ch in s.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c.is_control() => {
                // Skip control characters
            }
            c => output.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    
    #[test]
    fn test_json_format() {
        let entry = LogEntry::new(
            LogLevel::Info,
            "test".to_string(),
            "Hello, world!".to_string()
        );
        
        let json = JsonFormatter::format(&entry);
        assert!(json.contains("\"level\":\"INFO\""));
        assert!(json.contains("\"msg\":\"Hello, world!\""));
    }
}
