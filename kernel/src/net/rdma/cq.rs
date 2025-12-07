//! # RDMA Completion Queues
//! 
//! Completion Queue (CQ) management with:
//! - Poll-based completion
//! - Event-driven completion
//! - CQ resizing

use alloc::vec::Vec;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Completion Queue Events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CqEvent {
    CompletionAvailable,
    CqOverrun,
}

/// Completion Queue
pub struct CompletionQueueFull {
    cq_num: u32,
    completions: SpinLock<Vec<super::verbs::WorkCompletion>>,
    max_cqe: u32,
    
    // Statistics
    total_completions: AtomicU64,
    overruns: AtomicU64,
    
    // Event notification
    event_handler: Option<fn(CqEvent)>,
}

impl CompletionQueueFull {
    pub fn new(cq_num: u32, max_cqe: u32) -> Self {
        Self {
            cq_num,
            completions: SpinLock::new(Vec::with_capacity(max_cqe as usize)),
            max_cqe,
            total_completions: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            event_handler: None,
        }
    }
    
    /// Add completion
    pub fn add_completion(&self, wc: super::verbs::WorkCompletion) -> Result<(), CqError> {
        let mut completions = self.completions.lock();
        
        if completions.len() >= self.max_cqe as usize {
            self.overruns.fetch_add(1, Ordering::Relaxed);
            return Err(CqError::Overrun);
        }
        
        completions.push(wc);
        self.total_completions.fetch_add(1, Ordering::Relaxed);
        
        // Trigger event
        if let Some(handler) = self.event_handler {
            handler(CqEvent::CompletionAvailable);
        }
        
        Ok(())
    }
    
    /// Poll completions
    pub fn poll(&self, max_entries: usize) -> Vec<super::verbs::WorkCompletion> {
        let mut completions = self.completions.lock();
        let count = completions.len().min(max_entries);
        completions.drain(0..count).collect()
    }
    
    /// Set event handler
    pub fn set_event_handler(&mut self, handler: fn(CqEvent)) {
        self.event_handler = Some(handler);
    }
    
    /// Get statistics
    pub fn statistics(&self) -> CqStats {
        CqStats {
            total_completions: self.total_completions.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
            pending: self.completions.lock().len() as u64,
        }
    }
}

/// CQ statistics
#[derive(Debug, Clone, Copy)]
pub struct CqStats {
    pub total_completions: u64,
    pub overruns: u64,
    pub pending: u64,
}

/// CQ errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CqError {
    Overrun,
    InvalidCq,
}
