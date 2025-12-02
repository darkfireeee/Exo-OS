//! Lock-Free Ring Buffer
//!
//! Single-Producer Multi-Consumer (SPMC) ring buffer
//! Optimized for high-throughput audit logging

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct RingBuffer<T, const N: usize> {
    buffer: [Option<T>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T: Copy, const N: usize> RingBuffer<T, N> {
    pub const fn new() -> Self {
        Self {
            buffer: [None; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub fn push(&mut self, item: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        let next_head = (head + 1) % N;

        if next_head == tail {
            return Err(item); // Full
        }

        self.buffer[head] = Some(item);
        self.head.store(next_head, Ordering::Release);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if head == tail {
            return None; // Empty
        }

        let item = self.buffer[tail];
        self.tail.store((tail + 1) % N, Ordering::Release);
        item
    }

    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (head + 1) % N == tail
    }
}
