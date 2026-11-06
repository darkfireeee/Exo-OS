// ipc/stats/counters.rs — Global IPC statistical counters for Exo-OS
//
// This module provides centralized instrumentation for the entire IPC
// subsystem. Each significant event (send, receive, drop, error, latency...)
// is recorded via `IPC_STATS.record(StatEvent)`.
//
// Architecture:
//   - `StatEvent`: exhaustive enum of all measurable IPC events
//   - `IpcStats`: array of atomic counters indexed by StatEvent
//   - `IPC_STATS`: global static instance (zero allocation)
//   - `IpcStatsSnapshot`: immutable snapshot for inspection
//
// RULE STATS-01: all counters are AtomicU64 — never a mutex.
// RULE STATS-02: record() is inlined and O(1).
// RULE STATS-03: channel/ channels can import {IPC_STATS, StatEvent}
//                directly from this file.

use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// StatEvent — all measurable events of the IPC subsystem
// ---------------------------------------------------------------------------

/// Statistical IPC event identifier.
///
/// Each variant corresponds to a dedicated counter in `IpcStats`.
/// The ordering is stable — do not reorder without updating STAT_COUNT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum StatEvent {
    // --- Messages ---
    /// Message sent successfully
    MessageSent = 0,
    /// Message received successfully
    MessageReceived = 1,
    /// Message dropped (overflow/queue full)
    MessageDropped = 2,
    /// Message whose delivery failed (dead endpoint, etc.)
    MessageFailed = 3,

    // --- Channels ---
    /// Channel created
    ChannelCreated = 4,
    /// Channel closed
    ChannelClosed = 5,
    /// Channel destroyed
    ChannelDestroyed = 6,
    /// Blocking send (channel full, wait)
    ChannelSendBlocked = 7,
    /// Blocking receive (channel empty, wait)
    ChannelRecvBlocked = 8,
    /// Timeout during channel operation
    ChannelTimeout = 9,

    // --- Shared memory ---
    /// Successful SHM allocation
    ShmAllocated = 10,
    /// SHM free
    ShmFreed = 11,
    /// SHM allocation failure
    ShmAllocFailed = 12,
    /// SHM mapping into a process space
    ShmMapped = 13,
    /// SHM unmapping
    ShmUnmapped = 14,

    // --- Futex ---
    /// futex_wait() call
    FutexWait = 15,
    /// futex_wake() wakeup
    FutexWake = 16,
    /// futex_wait() timeout
    FutexTimeout = 17,
    /// futex requeue
    FutexRequeue = 18,

    // --- Ring buffers ---
    /// Successful push into a ring
    RingPush = 19,
    /// Successful pop from a ring
    RingPop = 20,
    /// Ring full (push failed)
    RingFull = 21,
    /// Ring empty (pop failed)
    RingEmpty = 22,

    // --- Endpoints ---
    /// Endpoint created
    EndpointCreated = 23,
    /// Endpoint destroyed
    EndpointDestroyed = 24,
    /// Connection between endpoints
    EndpointConnected = 25,
    /// Disconnection between endpoints
    EndpointDisconnected = 26,

    // --- Capabilities ---
    /// Capability check succeeded
    CapCheck = 27,
    /// Capability check failed (access denied)
    CapDenied = 28,
    /// Capability created
    CapCreated = 29,
    /// Capability revoked
    CapRevoked = 30,

    // --- RPC ---
    /// RPC call issued
    RpcCall = 31,
    /// RPC call completed
    RpcReturn = 32,
    /// RPC call timeout
    RpcTimeout = 33,
    /// RPC call protocol error
    RpcProtocolError = 34,

    // --- Synchronization ---
    /// Wake on IPC wait_queue
    WaitQueueWake = 35,
    /// wait_queue timeout
    WaitQueueTimeout = 36,
    /// Event set
    EventSet = 37,
    /// Event wait
    EventWait = 38,
    /// Barrier crossed
    BarrierCrossed = 39,
    /// Rendezvous met
    RendezvousMet = 40,

    // --- Transfer ---
    /// Descriptor transfer between processes
    DescriptorTransfer = 41,
    /// Zero-copy transfer
    ZeroCopyTransfer = 42,

    // --- Generic errors ---
    /// Generic IPC error
    GenericError = 43,
    /// Invalid parameter
    InvalidParam = 44,
}

/// Total number of statistical events
pub const STAT_COUNT: usize = 45;

// ---------------------------------------------------------------------------
// IpcStats — central table of atomic counters
// ---------------------------------------------------------------------------

/// Table of IPC statistical counters.
///
/// Indexed by `StatEvent as usize`. All counters are AtomicU64.
/// `record()` is inlined and boils down to an x86 LOCK `fetch_add`.
pub struct IpcStats {
    counters: [AtomicU64; STAT_COUNT],
}

// SAFETY: AtomicU64 is Sync
unsafe impl Sync for IpcStats {}
unsafe impl Send for IpcStats {}

impl IpcStats {
    /// Const constructor — usable in static
    pub const fn new() -> Self {
        // Local macro: generate AtomicU64::new(0) STAT_COUNT times.
        // We cannot use [expr; N] with non-Copy AtomicU64,
        // so we enumerate explicitly.
        Self {
            counters: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Records an IPC event (O(1), inlineable).
    #[inline(always)]
    pub fn record(&self, event: StatEvent) {
        // SAFETY: event as usize < STAT_COUNT guaranteed by bounded repr(usize)
        debug_assert!((event as usize) < STAT_COUNT);
        self.counters[event as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Records N occurrences of an event.
    #[inline(always)]
    pub fn record_n(&self, event: StatEvent, n: u64) {
        if n == 0 {
            return;
        }
        self.counters[event as usize].fetch_add(n, Ordering::Relaxed);
    }

    /// Reads current value of a counter.
    #[inline]
    pub fn get(&self, event: StatEvent) -> u64 {
        self.counters[event as usize].load(Ordering::Relaxed)
    }

    /// Resets a counter to zero.
    #[inline]
    pub fn reset(&self, event: StatEvent) {
        self.counters[event as usize].store(0, Ordering::Relaxed);
    }

    /// Resets all counters to zero.
    pub fn reset_all(&self) {
        for c in self.counters.iter() {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Produces an immutable snapshot (copy of current values).
    pub fn snapshot(&self) -> IpcStatsSnapshot {
        let mut values = [0u64; STAT_COUNT];
        for (i, c) in self.counters.iter().enumerate() {
            values[i] = c.load(Ordering::Relaxed);
        }
        IpcStatsSnapshot { values }
    }
}

// ---------------------------------------------------------------------------
// Global static instance
// ---------------------------------------------------------------------------

/// Global instance of IPC statistical counters.
///
/// Importable from any file in the IPC module:
/// ```rust
/// use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
/// IPC_STATS.record(StatEvent::MessageSent);
/// ```
pub static IPC_STATS: IpcStats = IpcStats::new();

// ---------------------------------------------------------------------------
// IpcStatsSnapshot — immutable snapshot
// ---------------------------------------------------------------------------

/// Instantaneous copy of all IPC counters.
///
/// Obtained via `IPC_STATS.snapshot()`. Values no longer change after
/// capture.
#[derive(Debug, Clone)]
pub struct IpcStatsSnapshot {
    values: [u64; STAT_COUNT],
}

impl IpcStatsSnapshot {
    /// Retrieves counter value in the snapshot.
    pub fn get(&self, event: StatEvent) -> u64 {
        self.values[event as usize]
    }

    /// Messages sent
    pub fn messages_sent(&self) -> u64 {
        self.get(StatEvent::MessageSent)
    }
    /// Messages received
    pub fn messages_received(&self) -> u64 {
        self.get(StatEvent::MessageReceived)
    }
    /// Messages dropped
    pub fn messages_dropped(&self) -> u64 {
        self.get(StatEvent::MessageDropped)
    }

    /// SHM allocations
    pub fn shm_allocated(&self) -> u64 {
        self.get(StatEvent::ShmAllocated)
    }
    /// SHM frees
    pub fn shm_freed(&self) -> u64 {
        self.get(StatEvent::ShmFreed)
    }

    /// RPC calls
    pub fn rpc_calls(&self) -> u64 {
        self.get(StatEvent::RpcCall)
    }
    /// RPC timeouts
    pub fn rpc_timeouts(&self) -> u64 {
        self.get(StatEvent::RpcTimeout)
    }

    /// Futex waits
    pub fn futex_waits(&self) -> u64 {
        self.get(StatEvent::FutexWait)
    }
    /// Futex wakes
    pub fn futex_wakes(&self) -> u64 {
        self.get(StatEvent::FutexWake)
    }

    /// Message drop rate (0-100 if sent > 0)
    pub fn message_drop_rate_pct(&self) -> u64 {
        let sent = self.messages_sent();
        if sent == 0 {
            return 0;
        }
        (self.messages_dropped() * 100) / sent
    }

    /// Formats a summary into a buffer of capacity `buf`.
    /// Returns the number of bytes written.
    pub fn format_summary(&self, buf: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct BufWriter<'a> {
            buf: &'a mut [u8],
            written: usize,
        }
        impl<'a> core::fmt::Write for BufWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let remaining = &mut self.buf[self.written..];
                let len = s.len().min(remaining.len());
                remaining[..len].copy_from_slice(&s.as_bytes()[..len]);
                self.written += len;
                Ok(())
            }
        }
        let mut w = BufWriter { buf, written: 0 };
        let _ = write!(
            w,
            "IPC_STATS: sent={} recv={} drop={} shm_alloc={} rpc={} futex_wait={} futex_wake={}",
            self.messages_sent(),
            self.messages_received(),
            self.messages_dropped(),
            self.shm_allocated(),
            self.rpc_calls(),
            self.futex_waits(),
            self.futex_wakes(),
        );
        w.written
    }
}
