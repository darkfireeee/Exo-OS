// kernel/src/ipc/ring/batch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BATCH TRANSFERS — batched transfers to amortize per-message overhead
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Instead of sending each message individually (fixed overhead per message:
// atomics, memory barriers), batching groups N messages into a single operation.
//
// BENEFITS:
//   • Reduces number of atomic operations from N -> 1.
//   • Improves cache locality (N messages in the same cache line).
//   • Mass unblocking: wakes a single waiter for N messages.
//
// CONSTRAINT: introduces buffering latency (FUSION_BATCH_THRESHOLD msgs
//              or FUSION_MAX_DELAY_TICKS scheduler ticks).
// ═══════════════════════════════════════════════════════════════════════════════

use super::spsc::SpscRing;
use crate::ipc::core::constants::FUSION_BATCH_THRESHOLD;
use crate::ipc::core::{IpcError, MsgFlags};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// BatchBuffer — local buffer for messages awaiting transmission
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum batch size (bounded by FUSION_BATCH_THRESHOLD).
pub const MAX_BATCH_SIZE: usize = 32;

/// A pending message in the batch buffer.
#[derive(Clone)]
pub struct BatchEntry {
    /// Message flags.
    pub flags: MsgFlags,
    /// Payload length.
    pub len: u16,
    /// Inline payload (max 4080 bytes, but in practice <= 512 for batching).
    pub data: [u8; 512],
}

impl BatchEntry {
    pub const fn zeroed() -> Self {
        Self {
            flags: MsgFlags(0),
            len: 0,
            data: [0u8; 512],
        }
    }
}

/// Producer-side batch buffer.
/// Accumulated locally, flushed to ring in a single operation.
pub struct BatchBuffer {
    /// Messages pending flush.
    entries: [BatchEntry; MAX_BATCH_SIZE],
    /// Number of accumulated messages.
    count: usize,
    /// Expiration tick: beyond this, forced flush.
    expire_tick: AtomicU64,
}

impl BatchBuffer {
    pub const fn new() -> Self {
        const ZERO: BatchEntry = BatchEntry::zeroed();
        Self {
            entries: [ZERO; MAX_BATCH_SIZE],
            count: 0,
            expire_tick: AtomicU64::new(0),
        }
    }

    /// Adds a message to the batch. Returns `true` if the batch should be flushed now.
    pub fn add(&mut self, src: &[u8], flags: MsgFlags, current_tick: u64) -> bool {
        // Payload limit per message in a batch.
        let len = src.len().min(512);
        if self.count == 0 {
            // First message of new batch — define expiration.
            self.expire_tick.store(
                current_tick + crate::ipc::core::constants::FUSION_MAX_DELAY_TICKS,
                Ordering::Relaxed,
            );
        }
        let idx = self.count;
        if idx < MAX_BATCH_SIZE {
            let entry = &mut self.entries[idx];
            entry.flags = flags;
            entry.len = len as u16;
            entry.data[..len].copy_from_slice(&src[..len]);
            self.count += 1;
        }
        // Flush if: batch full OR threshold reached.
        self.count >= FUSION_BATCH_THRESHOLD || self.count >= MAX_BATCH_SIZE
    }

    /// Checks if batch has expired (current tick > expire_tick).
    #[inline(always)]
    pub fn is_expired(&self, current_tick: u64) -> bool {
        self.count > 0 && current_tick >= self.expire_tick.load(Ordering::Relaxed)
    }

    /// Returns number of accumulated messages.
    #[inline(always)]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Flushes batch to an SPSC ring.
    /// Returns number of messages successfully sent.
    pub fn flush_to_ring(&mut self, ring: &SpscRing) -> usize {
        let mut sent = 0;
        for i in 0..self.count {
            let entry = &self.entries[i];
            let data = &entry.data[..entry.len as usize];
            match ring.push_copy(data, entry.flags) {
                Ok(_) => sent += 1,
                Err(_) => break, // ring full — retry at next tick
            }
        }
        // Shift unsent messages toward beginning.
        if sent > 0 && sent < self.count {
            for i in 0..(self.count - sent) {
                self.entries[i] = self.entries[sent + i].clone();
            }
        }
        self.count -= sent;
        if self.count == 0 {
            self.expire_tick.store(0, Ordering::Relaxed);
        }
        sent
    }

    /// Clears batch (discard).
    #[inline(always)]
    pub fn clear(&mut self) {
        self.count = 0;
        self.expire_tick.store(0, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchReceiver — batched reception from a ring
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a batched reception.
pub struct BatchReceiveResult {
    /// Number of received messages.
    pub received: usize,
    /// Total bytes transferred.
    pub bytes: usize,
}

/// Drains as many messages as possible from a ring into an array of buffers.
///
/// # Arguments
/// - `ring`    : source ring.
/// - `bufs`    : array of destination buffers; each entry is (ptr, len).
/// - `max`     : maximum number of messages to receive.
///
/// # Safety
/// Each buffer in `bufs` must be valid and mutable.
pub fn batch_receive(
    ring: &SpscRing,
    bufs: &mut [([u8; crate::ipc::core::MAX_MSG_SIZE], usize)],
    max: usize,
) -> BatchReceiveResult {
    let mut received = 0;
    let mut bytes = 0;
    let limit = max.min(bufs.len()).min(MAX_BATCH_SIZE);

    for buf in bufs.iter_mut().take(limit) {
        match ring.pop_into(&mut buf.0) {
            Ok((n, _flags)) => {
                buf.1 = n;
                bytes += n;
                received += 1;
            }
            Err(IpcError::QueueEmpty) => break,
            Err(_) => break,
        }
    }
    BatchReceiveResult { received, bytes }
}
