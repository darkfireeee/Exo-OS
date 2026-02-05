//! Structured logging system for Exo-OS
//!
//! High-performance async logger with:
//! - JSON Lines format
//! - Hierarchical spans
//! - Lock-free ring buffer
//! - File rotation
//! - Compression support

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

pub mod collector;
pub mod filter;
pub mod span;
pub mod formatter;
pub mod sink;

pub use collector::LogCollector;
pub use filter::{LogFilter, LogLevel};
pub use span::{Span, SpanId};

/// Global log sequence number
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

/// Log entry structure
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub seq: u64,
    pub timestamp: u64,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
    pub span_id: Option<SpanId>,
    pub fields: Vec<(String, String)>,
}

impl LogEntry {
    pub fn new(level: LogLevel, target: String, message: String) -> Self {
        Self {
            seq: LOG_SEQ.fetch_add(1, Ordering::Relaxed),
            timestamp: current_timestamp(),
            level,
            target,
            message,
            span_id: None,
            fields: Vec::new(),
        }
    }
    
    pub fn with_field(mut self, key: String, value: String) -> Self {
        self.fields.push((key, value));
        self
    }
}

/// Get current timestamp (stub - real implementation uses syscall)
fn current_timestamp() -> u64 {
    0 // TODO: syscall to get monotonic time
}

/// Initialize logger subsystem
pub fn init() {
    // Global logger initialization
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggerError {
    BufferFull,
    IoError,
    InvalidConfig,
}

pub type Result<T> = core::result::Result<T, LoggerError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_ordering() {
        assert!(Level::Trace < Level::Debug);
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info < Level::Warn);
    }
}
