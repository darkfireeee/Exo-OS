//! Pretty formatter for human-readable logs

use crate::{LogEntry, LogLevel};
use alloc::string::String;
use alloc::format;

pub struct PrettyFormatter;

impl PrettyFormatter {
    pub fn format(entry: &LogEntry) -> String {
        let level_str = match entry.level {
            LogLevel::Trace => "\x1b[90mTRC\x1b[0m", // Gray
            LogLevel::Debug => "\x1b[36mDBG\x1b[0m", // Cyan
            LogLevel::Info => "\x1b[32mINF\x1b[0m",  // Green
            LogLevel::Warn => "\x1b[33mWRN\x1b[0m",  // Yellow
            LogLevel::Error => "\x1b[31mERR\x1b[0m", // Red
            LogLevel::Fatal => "\x1b[35mFTL\x1b[0m", // Magenta
        };
        
        let mut output = format!(
            "[{}] {} {}: {}",
            entry.timestamp,
            level_str,
            entry.target,
            entry.message
        );
        
        if !entry.fields.is_empty() {
            output.push_str(" {");
            for (i, (key, value)) in entry.fields.iter().enumerate() {
                if i > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!("{}={}", key, value));
            }
            output.push('}');
        }
        
        output
    }
}
