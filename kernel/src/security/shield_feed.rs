// kernel/src/security/shield_feed.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ShieldFeed — security event ring kernel → exo_shield (TIER 3.1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// The NGAV server `exo_shield` (Ring 1) is reactive: it processes EVENT_REPORT
// and PMC_ANOMALY events sent to it. Without a feed, it is BLIND.
//
// This module provides the FEED: the kernel PUSHES (non-blocking, best-effort)
// security events into a bounded ring; the server DRAINS them via the
// `SYS_EXO_SHIELD_DRAIN` syscall and feeds them into its engine.
//
// DESIGN: ASYNCHRONOUS feed (ring + drain) rather than synchronous IPC per event —
//   • push cost ≈ short lock + 40-byte copy (never IPC in hot-path);
//   • no kernel→userspace deadlock (kernel never waits for the server);
//   • best-effort: if ring is full (server lagging), we DROP and count
//     (`DROPPED`) — kernel security (capability/zero_trust/exoledger) remains
//     the authority; the shield is a complementary detection layer.
//
// RULE SHIELDFEED-01: `push` NEVER blocks and NEVER panics.
// RULE SHIELDFEED-02: push sites are REAL security events (capability/IPC/syscall
//   denials, exec) — already logged elsewhere (exoledger).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Ring capacity (power of 2). 256 events awaiting drain.
const RING_SIZE: usize = 256;

/// Event category — mirrors `engine::EventType` on exo_shield side.
/// Stable values (feed ABI).
pub mod event_type {
    pub const PROCESS: u8 = 0;
    pub const SYSCALL: u8 = 1;
    pub const MEMORY: u8 = 2;
    pub const NETWORK: u8 = 3;
    pub const IPC: u8 = 4;
    pub const CAPABILITY: u8 = 5;
}

/// Threat level — mirrors `engine::ThreatLevel` on exo_shield side.
pub mod severity {
    pub const LOW: u8 = 0;
    pub const MEDIUM: u8 = 1;
    pub const HIGH: u8 = 2;
    pub const CRITICAL: u8 = 3;
}

/// Security event forwarded to shield. Stable `#[repr(C)]` layout = drain
/// syscall ABI (the server reads exactly this structure).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ShieldEvent {
    /// Target PID of the event.
    pub pid: u32,
    /// Category (`event_type::*`).
    pub event_type: u8,
    /// Severity (`severity::*`).
    pub severity: u8,
    pub _pad: [u8; 2],
    /// Operation code (syscall number, network opcode, exec action...).
    pub opcode: u32,
    pub _pad2: u32,
    /// Generic argument 0 (address, ppid, size...).
    pub arg0: u64,
    /// Generic argument 1.
    pub arg1: u64,
    /// Monotonic sequence number (order + loss detection on server side).
    pub seq: u64,
}

impl ShieldEvent {
    pub const fn zero() -> Self {
        Self {
            pid: 0,
            event_type: 0,
            severity: 0,
            _pad: [0; 2],
            opcode: 0,
            _pad2: 0,
            arg0: 0,
            arg1: 0,
            seq: 0,
        }
    }

    /// Ergonomic constructor for push call sites.
    #[inline]
    pub fn new(pid: u32, event_type: u8, severity: u8, opcode: u32, arg0: u64, arg1: u64) -> Self {
        Self {
            pid,
            event_type,
            severity,
            _pad: [0; 2],
            opcode,
            _pad2: 0,
            arg0,
            arg1,
            seq: 0,
        }
    }
}

/// Byte size of a `ShieldEvent` — exposed for syscall ABI.
pub const SHIELD_EVENT_SIZE: usize = core::mem::size_of::<ShieldEvent>();

static RING: Mutex<[ShieldEvent; RING_SIZE]> = Mutex::new([ShieldEvent::zero(); RING_SIZE]);
/// Write position (monotonic, modulo RING_SIZE on indexing).
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// Read/drain position (monotonic).
static TAIL: AtomicUsize = AtomicUsize::new(0);
/// Global sequence counter.
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Dropped events (ring full — server lagging or absent).
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Total pushed events.
static PUSHED: AtomicU64 = AtomicU64::new(0);

/// Pushes a security event to the shield. **Non-blocking, best-effort**:
/// if ring is full, the newest event is DROPPED (counted). Never panics
/// (RULE SHIELDFEED-01). Safe from any non-NMI context.
pub fn push(mut ev: ShieldEvent) {
    let mut ring = RING.lock();
    let head = HEAD.load(Ordering::Relaxed);
    let tail = TAIL.load(Ordering::Acquire);
    if head.wrapping_sub(tail) >= RING_SIZE {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    ev.seq = SEQ.fetch_add(1, Ordering::Relaxed);
    ring[head % RING_SIZE] = ev;
    HEAD.store(head.wrapping_add(1), Ordering::Release);
    PUSHED.fetch_add(1, Ordering::Relaxed);
}

/// Push shortcut (avoids constructing a `ShieldEvent` at call site).
#[inline]
pub fn push_event(pid: u32, event_type: u8, severity: u8, opcode: u32, arg0: u64, arg1: u64) {
    push(ShieldEvent::new(
        pid, event_type, severity, opcode, arg0, arg1,
    ));
}

/// Drains up to `out.len()` events into `out`. Returns the number of copied items.
/// Called by drain syscall (exo_shield server).
pub fn drain(out: &mut [ShieldEvent]) -> usize {
    let n = peek(out);
    advance(n);
    n
}

/// Peeks up to `out.len()` events into `out` without advancing the tail.
/// Pair with `advance(n)` once the caller finishes delivering them to userspace.
pub fn peek(out: &mut [ShieldEvent]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let ring = RING.lock();
    let head = HEAD.load(Ordering::Acquire);
    let mut tail = TAIL.load(Ordering::Relaxed);
    let mut n = 0;
    while tail != head && n < out.len() {
        out[n] = ring[tail % RING_SIZE];
        tail = tail.wrapping_add(1);
        n += 1;
    }
    n
}

/// Advances the drain tail by up to `count` already-peeked events.
pub fn advance(count: usize) {
    if count == 0 {
        return;
    }
    let _ring = RING.lock();
    let head = HEAD.load(Ordering::Acquire);
    let tail = TAIL.load(Ordering::Relaxed);
    let avail = head.wrapping_sub(tail);
    let step = count.min(avail);
    TAIL.store(tail.wrapping_add(step), Ordering::Release);
}

/// Feed statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct ShieldFeedStats {
    pub pushed: u64,
    pub dropped: u64,
    pub pending: usize,
}

pub fn stats() -> ShieldFeedStats {
    let head = HEAD.load(Ordering::Acquire);
    let tail = TAIL.load(Ordering::Acquire);
    ShieldFeedStats {
        pushed: PUSHED.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
        pending: head.wrapping_sub(tail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        // Drain everything to start clean (tests share static ring).
        let mut sink = [ShieldEvent::zero(); RING_SIZE];
        while drain(&mut sink) == RING_SIZE {}
    }

    #[test]
    fn push_then_drain_preserves_order_and_data() {
        reset();
        push_event(42, event_type::SYSCALL, severity::HIGH, 57, 0xAA, 0xBB);
        push_event(43, event_type::PROCESS, severity::CRITICAL, 59, 0xCC, 0xDD);

        let mut out = [ShieldEvent::zero(); 8];
        let n = drain(&mut out);
        assert!(n >= 2);
        // The last two of our pair must appear in order.
        let a = out[n - 2];
        let b = out[n - 1];
        assert_eq!(a.pid, 42);
        assert_eq!(a.event_type, event_type::SYSCALL);
        assert_eq!(a.opcode, 57);
        assert_eq!(b.pid, 43);
        assert_eq!(b.severity, severity::CRITICAL);
        assert!(b.seq > a.seq, "seq monotone");
    }

    #[test]
    fn drain_empty_returns_zero() {
        reset();
        let mut out = [ShieldEvent::zero(); 4];
        assert_eq!(drain(&mut out), 0);
    }

    #[test]
    fn overflow_drops_without_panic() {
        reset();
        // Fill beyond capacity -> surpluses are DROPPED, no panic.
        for i in 0..(RING_SIZE + 32) {
            push_event(i as u32, event_type::MEMORY, severity::LOW, 0, 0, 0);
        }
        assert!(stats().dropped >= 32, "les surplus doivent être comptés");
        // Ring contains at most RING_SIZE events.
        let mut out = [ShieldEvent::zero(); RING_SIZE];
        let n = drain(&mut out);
        assert!(n <= RING_SIZE);
    }
}
