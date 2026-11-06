// kernel/src/ipc/core/sequence.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SEQUENCE NUMBERS — Guaranteed ordering for IPC
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module guarantees causal ordering of messages in an IPC channel.
// Each channel maintains a sequence counter (u64) on the sender side.
// The receiver validates order and detects loss/duplicates via a
// sliding window bitmask.
//
// PERFORMANCE: all operations are O(1), lock-free on sender side.
// CONSTRAINT: zero heap allocation (conforms to NO-ALLOC zone).
// ═══════════════════════════════════════════════════════════════════════════════

use super::constants::SEQ_WINDOW_SIZE;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SeqSender — sender side (monotonically increasing)
// ─────────────────────────────────────────────────────────────────────────────

/// Sequence number generator — sender side.
/// A SeqSender is specific to a channel/direction.
#[repr(C, align(8))]
pub struct SeqSender {
    /// Sequence number of the next message to send.
    next_seq: AtomicU64,
}

impl SeqSender {
    /// Creates a new SeqSender starting at sequence 1.
    /// Sequence 0 is reserved as "invalid".
    pub const fn new() -> Self {
        Self {
            next_seq: AtomicU64::new(1),
        }
    }

    /// Allocates the next sequence number.
    /// Returns the value and increments it atomically.
    /// Thread-safe for multiple producers on the same channel.
    #[inline(always)]
    pub fn next(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Reads current number without incrementing.
    #[inline(always)]
    pub fn current(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed)
    }
}

impl Default for SeqSender {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqReceiver — receiver side (sliding window validation)
// ─────────────────────────────────────────────────────────────────────────────

/// Result of sequence number validation.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SeqCheck {
    /// Message in expected order: process immediately.
    InOrder,
    /// Message ahead of window: buffer it.
    Future { gap: u64 },
    /// Message already received (duplicate to reject).
    Duplicate,
    /// Message too old, outside window (reject).
    TooOld,
}

/// Receiver-side validator.
/// Maintains state required for order and duplicate detection.
///
/// CONSTRAINT: uses static storage only (no Vec).
/// The window is a bitmask of SEQ_WINDOW_BITS bits (u128 max 128 bits).
pub struct SeqReceiver {
    /// Next sequence number expected in order.
    expected: AtomicU64,
    /// Presence bitmask for window [expected .. expected + WINDOW).
    /// Bit i = 1 if message expected+i has already been received.
    window: AtomicU64,
    /// Counter of detected out-of-order messages.
    out_of_order_count: AtomicU64,
    /// Counter of rejected duplicate messages.
    duplicate_count: AtomicU64,
}

impl SeqReceiver {
    /// Creates a receiver expecting sequence 1 first.
    pub const fn new() -> Self {
        Self {
            expected: AtomicU64::new(1),
            window: AtomicU64::new(0),
            out_of_order_count: AtomicU64::new(0),
            duplicate_count: AtomicU64::new(0),
        }
    }

    /// Validates and records receipt of a sequence number.
    ///
    /// # Return
    /// - `InOrder`     : message in order, process now.
    /// - `Future {gap}`: message ahead by `gap` positions, buffer it.
    /// - `Duplicate`   : already received, ignore.
    /// - `TooOld`      : outside window, ignore.
    ///
    /// # Concurrency note
    /// This function is not atomic across the entire operation.
    /// It is designed for single-consumer usage (SPSC or lock-protected).
    pub fn check_and_advance(&self, seq: u64) -> SeqCheck {
        let expected = self.expected.load(Ordering::Acquire);

        if seq == expected {
            // Fast path: message in order.
            // Advance window by consuming consecutive bits already received.
            let mut new_expected = expected + 1;
            let mut win = self.window.load(Ordering::Relaxed);

            // Advance as long as the next bit is already present.
            while (win & 1) != 0 {
                win >>= 1;
                new_expected += 1;
            }

            self.window.store(win, Ordering::Relaxed);
            self.expected.store(new_expected, Ordering::Release);
            SeqCheck::InOrder
        } else if seq > expected {
            let gap = seq - expected;
            if gap >= SEQ_WINDOW_SIZE {
                // Too far in the future — cannot buffer.
                SeqCheck::Future { gap }
            } else {
                // Inside the window — mark the bit.
                let bit = 1u64 << (gap - 1);
                let win = self.window.fetch_or(bit, Ordering::Relaxed);
                if win & bit != 0 {
                    // Bit already set -> duplicate.
                    self.duplicate_count.fetch_add(1, Ordering::Relaxed);
                    SeqCheck::Duplicate
                } else {
                    self.out_of_order_count.fetch_add(1, Ordering::Relaxed);
                    SeqCheck::Future { gap }
                }
            }
        } else {
            // seq < expected — message already past/consumed.
            // The Exo-OS protocol classifies this case as `TooOld`; `Duplicate` is
            // reserved for future messages already buffered in the window.
            let _behind = expected - seq;
            SeqCheck::TooOld
        }
    }

    /// Returns the next expected number.
    #[inline(always)]
    pub fn expected(&self) -> u64 {
        self.expected.load(Ordering::Relaxed)
    }

    /// Statistics: out-of-order messages.
    #[inline(always)]
    pub fn out_of_order_count(&self) -> u64 {
        self.out_of_order_count.load(Ordering::Relaxed)
    }

    /// Statistics: detected duplicates.
    #[inline(always)]
    pub fn duplicate_count(&self) -> u64 {
        self.duplicate_count.load(Ordering::Relaxed)
    }

    /// Resets the receiver to the given sequence.
    /// To be used upon reconnection or channel reset.
    pub fn reset(&self, start_seq: u64) {
        self.expected.store(start_seq, Ordering::Release);
        self.window.store(0, Ordering::Relaxed);
        self.out_of_order_count.store(0, Ordering::Relaxed);
        self.duplicate_count.store(0, Ordering::Relaxed);
    }
}

impl Default for SeqReceiver {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqPair — sender/receiver pair for a unidirectional channel
// ─────────────────────────────────────────────────────────────────────────────

/// A complete unidirectional channel for sequence management.
pub struct SeqPair {
    pub sender: SeqSender,
    pub receiver: SeqReceiver,
}

impl SeqPair {
    pub const fn new() -> Self {
        Self {
            sender: SeqSender::new(),
            receiver: SeqReceiver::new(),
        }
    }
}

impl Default for SeqPair {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inorder_sequence() {
        let recv = SeqReceiver::new();
        assert_eq!(recv.check_and_advance(1), SeqCheck::InOrder);
        assert_eq!(recv.check_and_advance(2), SeqCheck::InOrder);
        assert_eq!(recv.check_and_advance(3), SeqCheck::InOrder);
        assert_eq!(recv.expected(), 4);
    }

    #[test]
    fn test_out_of_order_then_gap_fill() {
        let recv = SeqReceiver::new();
        // Seq 2 arrives before seq 1.
        assert!(matches!(
            recv.check_and_advance(2),
            SeqCheck::Future { gap: 1 }
        ));
        // Now seq 1 arrives -> must trigger InOrder and advance.
        assert_eq!(recv.check_and_advance(1), SeqCheck::InOrder);
        // expected must have jumped to 3 (2 was already in window).
        assert_eq!(recv.expected(), 3);
    }

    #[test]
    fn test_duplicate_detection() {
        let recv = SeqReceiver::new();
        assert_eq!(recv.check_and_advance(1), SeqCheck::InOrder);
        // Resend the same number.
        assert_eq!(recv.check_and_advance(1), SeqCheck::TooOld);
        assert_eq!(recv.duplicate_count(), 0); // TooOld, not Duplicate
    }
}
