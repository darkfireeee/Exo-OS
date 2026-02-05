//! Tracing spans for hierarchical logging

use core::num::NonZeroU64;
use core::sync::atomic::{AtomicU64, Ordering};

static SPAN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SpanId(NonZeroU64);

impl SpanId {
    pub fn new() -> Self {
        let id = SPAN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(unsafe { NonZeroU64::new_unchecked(id) })
    }
    
    pub fn as_u64(self) -> u64 {
        self.0.get()
    }
}

pub struct Span {
    id: SpanId,
    name: &'static str,
}

impl Span {
    pub fn new(name: &'static str) -> Self {
        Self {
            id: SpanId::new(),
            name,
        }
    }
    
    pub fn id(&self) -> SpanId {
        self.id
    }
    
    pub fn name(&self) -> &'static str {
        self.name
    }
    
    pub fn enter(&self) {
        // TODO: Set thread-local current span
    }
    
    pub fn exit(&self) {
        // TODO: Clear thread-local current span
    }
}
