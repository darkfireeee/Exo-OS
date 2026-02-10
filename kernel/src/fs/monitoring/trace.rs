//! Trace - Debug Event Tracing
//!
//! Lightweight tracing for debugging filesystem operations.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::RwLock;

/// Maximum trace events to keep in buffer
const MAX_TRACE_EVENTS: usize = 10000;

/// Trace event severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TraceLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

/// A single trace event
#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub timestamp: u64,
    pub level: TraceLevel,
    pub message: String,
}

/// Trace buffer - circular buffer for events
struct TraceBuffer {
    events: RwLock<Vec<TraceEvent>>,
    enabled: AtomicBool,
    seq: AtomicU64,
}

impl TraceBuffer {
    const fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
            enabled: AtomicBool::new(true),
            seq: AtomicU64::new(0),
        }
    }

    fn record(&self, level: TraceLevel, message: String) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let timestamp = self.seq.fetch_add(1, Ordering::Relaxed);
        let event = TraceEvent {
            timestamp,
            level,
            message,
        };

        let mut events = self.events.write();

        // Circular buffer - drop oldest if full
        if events.len() >= MAX_TRACE_EVENTS {
            events.remove(0);
        }

        events.push(event);
    }

    fn get_events(&self, level: Option<TraceLevel>) -> Vec<TraceEvent> {
        let events = self.events.read();

        match level {
            Some(min_level) => events
                .iter()
                .filter(|e| e.level as u8 >= min_level as u8)
                .cloned()
                .collect(),
            None => events.clone(),
        }
    }

    fn clear(&self) {
        let mut events = self.events.write();
        events.clear();
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

/// Global trace buffer
static GLOBAL_TRACE: TraceBuffer = TraceBuffer::new();

/// Trace an event with default Info level
pub fn trace(event: &str) {
    trace_level(TraceLevel::Info, event);
}

/// Trace an event with specific level
pub fn trace_level(level: TraceLevel, event: &str) {
    GLOBAL_TRACE.record(level, String::from(event));
}

/// Trace a debug event
pub fn trace_debug(event: &str) {
    trace_level(TraceLevel::Debug, event);
}

/// Trace a warning event
pub fn trace_warn(event: &str) {
    trace_level(TraceLevel::Warn, event);
}

/// Trace an error event
pub fn trace_error(event: &str) {
    trace_level(TraceLevel::Error, event);
}

/// Get all trace events, optionally filtered by minimum level
pub fn get_events(min_level: Option<TraceLevel>) -> Vec<TraceEvent> {
    GLOBAL_TRACE.get_events(min_level)
}

/// Clear all trace events
pub fn clear() {
    GLOBAL_TRACE.clear();
}

/// Enable/disable tracing
pub fn set_enabled(enabled: bool) {
    GLOBAL_TRACE.set_enabled(enabled);
    log::debug!("Trace subsystem {}", if enabled { "enabled" } else { "disabled" });
}

/// Check if tracing is enabled
pub fn is_enabled() -> bool {
    GLOBAL_TRACE.enabled.load(Ordering::Relaxed)
}

/// Get trace buffer statistics
pub fn stats() -> TraceStats {
    let events = GLOBAL_TRACE.events.read();
    let total = events.len();

    let mut debug_count = 0;
    let mut info_count = 0;
    let mut warn_count = 0;
    let mut error_count = 0;

    for event in events.iter() {
        match event.level {
            TraceLevel::Debug => debug_count += 1,
            TraceLevel::Info => info_count += 1,
            TraceLevel::Warn => warn_count += 1,
            TraceLevel::Error => error_count += 1,
        }
    }

    TraceStats {
        total_events: total,
        debug_count,
        info_count,
        warn_count,
        error_count,
        enabled: GLOBAL_TRACE.enabled.load(Ordering::Relaxed),
    }
}

/// Trace buffer statistics
#[derive(Debug, Clone, Copy)]
pub struct TraceStats {
    pub total_events: usize,
    pub debug_count: usize,
    pub info_count: usize,
    pub warn_count: usize,
    pub error_count: usize,
    pub enabled: bool,
}

/// Initialize trace subsystem
pub fn init() {
    GLOBAL_TRACE.set_enabled(true);
    log::debug!("Trace subsystem initialized (buffer size: {})", MAX_TRACE_EVENTS);
}
