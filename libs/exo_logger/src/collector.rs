//! Log event collection and buffering

use crate::{LogEntry, Result, LoggerError};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Lock-free log collector with ring buffer
pub struct LogCollector {
    buffer: Vec<Option<LogEntry>>,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl LogCollector {
    /// Create new collector with capacity
    pub fn new(capacity: usize) -> Self {
        let mut buffer = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buffer.push(None);
        }
        
        Self {
            buffer,
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
    
    /// Push log entry (non-blocking)
    pub fn push(&mut self, entry: LogEntry) -> Result<()> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        
        let next_head = (head + 1) % self.capacity;
        if next_head == tail {
            return Err(LoggerError::BufferFull);
        }
        
        self.buffer[head] = Some(entry);
        self.head.store(next_head, Ordering::Release);
        Ok(())
    }
    
    /// Pop log entry
    pub fn pop(&mut self) -> Option<LogEntry> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        
        if tail == head {
            return None;
        }
        
        let entry = self.buffer[tail].take();
        let next_tail = (tail + 1) % self.capacity;
        self.tail.store(next_tail, Ordering::Release);
        entry
    }
    
    /// Drain all entries
    pub fn drain(&mut self) -> Vec<LogEntry> {
        let mut entries = Vec::new();
        while let Some(entry) = self.pop() {
            entries.push(entry);
        }
        entries
    }
    
    /// Get buffered count
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (head + self.capacity - tail) % self.capacity
    }
    
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogLevel;
    use alloc::string::ToString;
    
    #[test]
    fn test_ring_buffer() {
        let mut collector = LogCollector::new(4);
        
        let entry1 = LogEntry::new(LogLevel::Info, "test".to_string(), "msg1".to_string());
        let entry2 = LogEntry::new(LogLevel::Debug, "test".to_string(), "msg2".to_string());
        
        assert!(collector.push(entry1).is_ok());
        assert!(collector.push(entry2).is_ok());
        assert_eq!(collector.len(), 2);
        
        let popped = collector.pop().unwrap();
        assert_eq!(popped.message, "msg1");
        assert_eq!(collector.len(), 1);
    }
}
