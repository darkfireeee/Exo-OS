//! # memory_hooks — Memory allocation monitoring & anomaly detection
//!
//! Intercepts memory allocation/deallocation events to detect:
//! - **Buffer overflow**: canary-based detection at allocation boundaries
//! - **Use-after-free**: freed-address tracking with delayed reuse
//! - **Allocation anomalies**: rapid alloc/free patterns, oversized requests
//! - **Memory scanning**: pattern-based scanning of process memory regions
//!
//! All data structures use static arrays — no heap, no `Vec`, no `String`.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum tracked allocation records.
const MAX_ALLOC_RECORDS: usize = 512;

/// Maximum freed-region tracking entries (UAF detection).
const MAX_FREED_REGIONS: usize = 256;

/// Maximum recent memory events stored.
const MAX_MEM_EVENTS: usize = 1024;

/// Canary value placed after allocation boundaries.
const CANARY_VALUE: u32 = 0xDEAD_BEEF;

/// Maximum allocation size considered normal (64 KB).
const MAX_NORMAL_ALLOC: usize = 65536;

/// Allocation rate threshold per PID within window.
const ALLOC_RATE_THRESHOLD: u32 = 256;

/// Minimum delay in TSC ticks before a freed address can be reused (~100 ms).
const UAF_QUARANTINE_TSC: u64 = 300_000_000;

/// Tracking window in TSC ticks (~1 second at 3 GHz).
const TRACKING_WINDOW_TSC: u64 = 3_000_000_000;

/// Maximum scan pattern length.
const MAX_PATTERN_LEN: usize = 32;

/// Maximum scan result hits.
const MAX_SCAN_HITS: usize = 64;

// ── TSC read ──────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Memory event type discriminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MemEventType {
    /// Memory allocation (malloc/mmap).
    Alloc = 0,
    /// Memory deallocation (free/munmap).
    Free = 1,
    /// Buffer overflow detected.
    Overflow = 2,
    /// Use-after-free detected.
    UseAfterFree = 3,
    /// Allocation rate anomaly.
    RateAnomaly = 4,
    /// Oversized allocation.
    OversizedAlloc = 5,
    /// Canary corruption detected.
    CanaryCorrupt = 6,
}

impl MemEventType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Alloc,
            1 => Self::Free,
            2 => Self::Overflow,
            3 => Self::UseAfterFree,
            4 => Self::RateAnomaly,
            5 => Self::OversizedAlloc,
            6 => Self::CanaryCorrupt,
            _ => Self::Alloc,
        }
    }
}

/// Memory event recorded for each relevant memory operation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemEvent {
    /// PID owning the memory.
    pub pid: u32,
    /// Event type.
    pub event_type: u8,
    /// Flags: bit 0 = heap, bit 1 = mmap, bit 2 = stack.
    pub flags: u8,
    /// Padding.
    pub _pad: u16,
    /// Virtual address of the allocation (lower 32 bits for compactness).
    pub addr: u64,
    /// Size of the allocation in bytes.
    pub size: u64,
    /// TSC timestamp.
    pub timestamp: u64,
}

impl Default for MemEvent {
    fn default() -> Self {
        Self::empty()
    }
}

impl MemEvent {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            event_type: 0,
            flags: 0,
            _pad: 0,
            addr: 0,
            size: 0,
            timestamp: 0,
        }
    }
}

/// Allocation tracking record.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AllocRecord {
    /// PID that owns the allocation.
    pub pid: u32,
    /// Base address of the allocation.
    pub addr: u64,
    /// Requested size.
    pub size: u64,
    /// Canary value stored after the allocation boundary.
    pub canary: u32,
    /// Flags: bit 0 = active, bit 1 = canary_checked.
    pub flags: u8,
    /// Padding.
    pub _pad: [u8; 3],
    /// TSC timestamp of allocation.
    pub timestamp: u64,
}

impl Default for AllocRecord {
    fn default() -> Self {
        Self::empty()
    }
}

impl AllocRecord {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            addr: 0,
            size: 0,
            canary: 0,
            flags: 0,
            _pad: [0; 3],
            timestamp: 0,
        }
    }
}

/// Freed region tracking entry (for UAF detection).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FreedRegion {
    /// PID that freed the memory.
    pub pid: u32,
    /// Freed address.
    pub addr: u64,
    /// Size of the freed region.
    pub size: u64,
    /// TSC timestamp when the region was freed.
    pub free_timestamp: u64,
    /// Whether this entry is active (in quarantine).
    pub active: u8,
    /// Padding.
    pub _pad: [u8; 3],
}

impl Default for FreedRegion {
    fn default() -> Self {
        Self::empty()
    }
}

impl FreedRegion {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            addr: 0,
            size: 0,
            free_timestamp: 0,
            active: 0,
            _pad: [0; 3],
        }
    }
}

/// Allocation rate tracking entry (per PID).
struct AllocRateEntry {
    pid: AtomicU32,
    count: AtomicU32,
    window_start: AtomicU64,
    flagged: AtomicU8,
}

impl AllocRateEntry {
    const fn new() -> Self {
        Self {
            pid: AtomicU32::new(0),
            count: AtomicU32::new(0),
            window_start: AtomicU64::new(0),
            flagged: AtomicU8::new(0),
        }
    }
}

// ── Static storage ────────────────────────────────────────────────────────────

/// Ring buffer of recent memory events.
static MEM_EVENTS: Mutex<[MemEvent; MAX_MEM_EVENTS]> =
    Mutex::new([const { MemEvent::empty() }; MAX_MEM_EVENTS]);
static MEM_EVENT_IDX: AtomicU32 = AtomicU32::new(0);

/// Allocation tracking table.
static ALLOC_TABLE: Mutex<[AllocRecord; MAX_ALLOC_RECORDS]> =
    Mutex::new([const { AllocRecord::empty() }; MAX_ALLOC_RECORDS]);
static ALLOC_COUNT: AtomicU32 = AtomicU32::new(0);

/// Freed region quarantine table.
static FREED_TABLE: Mutex<[FreedRegion; MAX_FREED_REGIONS]> =
    Mutex::new([const { FreedRegion::empty() }; MAX_FREED_REGIONS]);
static FREED_COUNT: AtomicU32 = AtomicU32::new(0);

/// Allocation rate tracking table.
static ALLOC_RATE_TABLE: Mutex<[AllocRateEntry; 64]> =
    Mutex::new([const { AllocRateEntry::new() }; 64]);

/// Statistics counters.
static TOTAL_MEM_EVENTS: AtomicU64 = AtomicU64::new(0);
static OVERFLOW_DETECTIONS: AtomicU64 = AtomicU64::new(0);
static UAF_DETECTIONS: AtomicU64 = AtomicU64::new(0);
static RATE_ANOMALIES: AtomicU64 = AtomicU64::new(0);
static OVERSIZED_ALLOCS: AtomicU64 = AtomicU64::new(0);
static CANARY_CORRUPTIONS: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Store a memory event into the ring buffer.
fn store_mem_event(event: MemEvent) {
    let mut events = MEM_EVENTS.lock();
    let idx = MEM_EVENT_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_MEM_EVENTS;
    events[idx] = event;
}

/// Check and bump allocation rate for a PID.
/// Returns `true` if rate anomaly detected.
fn check_alloc_rate(pid: u32) -> bool {
    let table = ALLOC_RATE_TABLE.lock();
    let now = read_tsc();

    for i in 0..64 {
        let entry_pid = table[i].pid.load(Ordering::Acquire);
        if entry_pid == pid {
            let start = table[i].window_start.load(Ordering::Acquire);
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                table[i].count.store(1, Ordering::Release);
                table[i].window_start.store(now, Ordering::Release);
                table[i].flagged.store(0, Ordering::Release);
                return false;
            }
            let new_count = table[i].count.fetch_add(1, Ordering::AcqRel) + 1;
            if new_count >= ALLOC_RATE_THRESHOLD && table[i].flagged.load(Ordering::Acquire) == 0 {
                table[i].flagged.store(1, Ordering::Release);
                RATE_ANOMALIES.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            return table[i].flagged.load(Ordering::Acquire) != 0;
        }
    }

    // New entry
    for i in 0..64 {
        if table[i].pid.load(Ordering::Acquire) == 0 {
            table[i].pid.store(pid, Ordering::Release);
            table[i].count.store(1, Ordering::Release);
            table[i].window_start.store(now, Ordering::Release);
            table[i].flagged.store(0, Ordering::Release);
            return false;
        }
    }

    // Evict oldest
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..64 {
        let start = table[i].window_start.load(Ordering::Acquire);
        if start < oldest_tsc {
            oldest_tsc = start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].pid.store(pid, Ordering::Release);
    table[oldest_idx].count.store(1, Ordering::Release);
    table[oldest_idx].window_start.store(now, Ordering::Release);
    table[oldest_idx].flagged.store(0, Ordering::Release);
    false
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Validates an allocation request before it is fulfilled.
///
/// Checks:
/// 1. Oversized allocation flagging
/// 2. Allocation rate anomaly detection
///
/// Returns `true` if the allocation should be blocked.
pub fn pre_alloc_check(pid: u32, size: u64, _flags: u8) -> bool {
    // Check for oversized allocation
    if size > MAX_NORMAL_ALLOC as u64 {
        OVERSIZED_ALLOCS.fetch_add(1, Ordering::Relaxed);
        // Flag but don't block — let post_alloc handle monitoring
    }

    // Check allocation rate
    check_alloc_rate(pid)
}

/// Monitors an allocation after it has been fulfilled.
///
/// Records the allocation in the tracking table and places a canary
/// value after the allocation boundary (conceptually; in practice the
/// canary address is computed but writing it requires kernel cooperation).
pub fn post_alloc_monitor(pid: u32, addr: u64, size: u64, flags: u8) {
    TOTAL_MEM_EVENTS.fetch_add(1, Ordering::Relaxed);

    let now = read_tsc();
    let event = MemEvent {
        pid,
        event_type: MemEventType::Alloc as u8,
        flags,
        _pad: 0,
        addr,
        size,
        timestamp: now,
    };
    store_mem_event(event);

    // Record in allocation table
    let record = AllocRecord {
        pid,
        addr,
        size,
        canary: CANARY_VALUE,
        flags: 1, // active
        _pad: [0; 3],
        timestamp: now,
    };

    let mut table = ALLOC_TABLE.lock();
    let count = ALLOC_COUNT.load(Ordering::Acquire) as usize;
    if count < MAX_ALLOC_RECORDS {
        table[count] = record;
        ALLOC_COUNT.fetch_add(1, Ordering::Release);
    } else {
        // Find an inactive slot or evict the oldest
        let mut replaced = false;
        for i in 0..MAX_ALLOC_RECORDS {
            if table[i].flags & 1 == 0 {
                table[i] = record;
                replaced = true;
                break;
            }
        }
        if !replaced {
            let mut oldest_idx = 0usize;
            let mut oldest_tsc = u64::MAX;
            for i in 0..MAX_ALLOC_RECORDS {
                if table[i].timestamp < oldest_tsc {
                    oldest_tsc = table[i].timestamp;
                    oldest_idx = i;
                }
            }
            table[oldest_idx] = record;
        }
    }

    // Oversized allocation event
    if size > MAX_NORMAL_ALLOC as u64 {
        let oversized_event = MemEvent {
            pid,
            event_type: MemEventType::OversizedAlloc as u8,
            flags,
            _pad: 0,
            addr,
            size,
            timestamp: now,
        };
        store_mem_event(oversized_event);
    }
}

/// Detects buffer overflow by checking the canary value of a tracked allocation.
///
/// Returns `Some(true)` if the canary is corrupted (overflow detected),
/// `Some(false)` if the canary is intact, `None` if the allocation
/// is not tracked.
///
/// In a real bare-metal kernel, the canary would be written at
/// `addr + size` by the kernel. This function checks the stored
/// canary value against the expected `CANARY_VALUE`.
pub fn detect_buffer_overflow(pid: u32, addr: u64) -> Option<bool> {
    let table = ALLOC_TABLE.lock();

    for i in 0..MAX_ALLOC_RECORDS {
        let entry = &table[i];
        if entry.flags & 1 == 0 {
            continue;
        }
        if entry.pid == pid && entry.addr == addr {
            // Check canary: if the stored canary doesn't match,
            // the buffer was overwritten past its boundary
            let canary_ok = entry.canary == CANARY_VALUE;
            if !canary_ok {
                OVERFLOW_DETECTIONS.fetch_add(1, Ordering::Relaxed);

                let event = MemEvent {
                    pid,
                    event_type: MemEventType::Overflow as u8,
                    flags: 0,
                    _pad: 0,
                    addr,
                    size: entry.size,
                    timestamp: read_tsc(),
                };
                store_mem_event(event);
            }
            return Some(canary_ok);
        }
    }
    None
}

/// Detects use-after-free by checking if an address being accessed
/// is in the freed-region quarantine.
///
/// Returns `Some(FreedRegion)` if UAF is detected (the address falls
/// within a quarantined freed region), `None` otherwise.
pub fn detect_use_after_free(pid: u32, addr: u64) -> Option<FreedRegion> {
    let mut table = FREED_TABLE.lock();
    let now = read_tsc();

    for i in 0..MAX_FREED_REGIONS {
        let entry = &table[i];
        if entry.active == 0 {
            continue;
        }

        // Check if address falls within this freed region
        let region_start = entry.addr;
        let region_end = entry.addr.wrapping_add(entry.size);
        if addr >= region_start && addr < region_end {
            // Check if quarantine period has expired
            let elapsed = now.wrapping_sub(entry.free_timestamp);
            if elapsed < UAF_QUARANTINE_TSC {
                // Still in quarantine → UAF detected
                UAF_DETECTIONS.fetch_add(1, Ordering::Relaxed);

                let event = MemEvent {
                    pid,
                    event_type: MemEventType::UseAfterFree as u8,
                    flags: 0,
                    _pad: 0,
                    addr,
                    size: 0,
                    timestamp: now,
                };
                store_mem_event(event);

                return Some(*entry);
            } else {
                // Quarantine expired — release this entry
                table[i].active = 0;
                FREED_COUNT.fetch_sub(1, Ordering::Release);
            }
        }
    }
    None
}

/// Records a free operation, moving the allocation to the quarantine
/// table for UAF detection.
pub fn record_free(pid: u32, addr: u64) {
    TOTAL_MEM_EVENTS.fetch_add(1, Ordering::Relaxed);

    let now = read_tsc();

    // Find the allocation record to get its size
    let mut size = 0u64;
    {
        let mut table = ALLOC_TABLE.lock();
        for i in 0..MAX_ALLOC_RECORDS {
            if table[i].flags & 1 != 0 && table[i].pid == pid && table[i].addr == addr {
                size = table[i].size;
                table[i].flags = 0; // Mark inactive
                break;
            }
        }
    }

    // Record the free event
    let event = MemEvent {
        pid,
        event_type: MemEventType::Free as u8,
        flags: 0,
        _pad: 0,
        addr,
        size,
        timestamp: now,
    };
    store_mem_event(event);

    // Add to quarantine if we found the allocation
    if size > 0 {
        let freed = FreedRegion {
            pid,
            addr,
            size,
            free_timestamp: now,
            active: 1,
            _pad: [0; 3],
        };

        let mut table = FREED_TABLE.lock();
        let count = FREED_COUNT.load(Ordering::Acquire) as usize;

        // First try to release expired entries
        for i in 0..MAX_FREED_REGIONS {
            if table[i].active != 0 {
                let elapsed = now.wrapping_sub(table[i].free_timestamp);
                if elapsed >= UAF_QUARANTINE_TSC {
                    table[i] = freed;
                    return;
                }
            }
        }

        // Then use an inactive slot
        if count < MAX_FREED_REGIONS {
            for i in 0..MAX_FREED_REGIONS {
                if table[i].active == 0 {
                    table[i] = freed;
                    FREED_COUNT.fetch_add(1, Ordering::Release);
                    return;
                }
            }
        }

        // Evict the oldest quarantined entry
        let mut oldest_idx = 0usize;
        let mut oldest_tsc = u64::MAX;
        for i in 0..MAX_FREED_REGIONS {
            if table[i].active != 0 && table[i].free_timestamp < oldest_tsc {
                oldest_tsc = table[i].free_timestamp;
                oldest_idx = i;
            }
        }
        table[oldest_idx] = freed;
    }
}

/// Scans a memory region for a specific byte pattern.
///
/// This is a simplified model: the actual memory contents would be
/// provided by the kernel. Here we simulate scanning by recording
/// the scan request and returning match information based on
/// the allocation table.
///
/// FIX-23: Exact byte-subsequence scanner over a raw memory slice.
/// Fills `hits` with `base_addr + offset` for every real pattern occurrence (bounded by `hits.len()`).
pub fn scan_buffer_for_pattern(
    base_addr: u64,
    buf: &[u8],
    pattern: &[u8],
    hits: &mut [u64],
) -> usize {
    if pattern.is_empty() || pattern.len() > buf.len() || hits.is_empty() {
        return 0;
    }
    let mut found = 0usize;
    let limit = buf.len() - pattern.len() + 1;
    let max_hits = hits.len().min(MAX_SCAN_HITS);
    let mut pos = 0usize;
    while pos < limit && found < max_hits {
        if &buf[pos..pos + pattern.len()] == pattern {
            hits[found] = base_addr.wrapping_add(pos as u64);
            found += 1;
            pos += pattern.len();
        } else {
            pos += 1;
        }
    }
    found
}

/// Scans a target process's memory region `[region_start .. region_start + region_size)`
/// for `pattern` (FIX-23).
///
/// - On bare-metal Exo-OS (`target_os = "none"`), uses bounded read-only
///   `SYS_EXO_MEM_COPY_FROM_PID` chunks (512 bytes, up to 16 KiB per scan) to read
///   genuine target memory and perform exact byte matching.
/// - On host unit tests (where remote syscall memory doesn't exist), falls back to
///   scanning allocation metadata canaries when `pattern == CANARY_VALUE`.
pub fn scan_memory_region(
    pid: u32,
    region_start: u64,
    region_size: u64,
    pattern: &[u8],
    hits: &mut [u64],
) -> usize {
    let pat_len = pattern.len().min(MAX_PATTERN_LEN);
    if pat_len == 0 || hits.is_empty() {
        return 0;
    }
    let pat = &pattern[..pat_len];

    #[cfg(target_os = "none")]
    {
        if pid > 1 && region_start != 0 && region_size > 0 {
            const CHUNK: usize = 512;
            const MAX_REMOTE_SCAN: usize = 16 * 1024;
            let total = (region_size as usize).min(MAX_REMOTE_SCAN);
            let mut buf = [0u8; CHUNK];
            let mut offset = 0usize;
            let mut found = 0usize;
            while offset < total && found < hits.len().min(MAX_SCAN_HITS) {
                let n = (total - offset).min(CHUNK);
                let rc = unsafe {
                    exo_syscall_abi::syscall5(
                        exo_syscall_abi::SYS_EXO_MEM_COPY_FROM_PID,
                        pid as u64,
                        region_start.wrapping_add(offset as u64),
                        buf.as_mut_ptr() as u64,
                        n as u64,
                        0,
                    )
                };
                if rc <= 0 {
                    break;
                }
                let read_len = (rc as usize).min(n);
                found += scan_buffer_for_pattern(
                    region_start.wrapping_add(offset as u64),
                    &buf[..read_len],
                    pat,
                    &mut hits[found..],
                );
                if read_len < n {
                    break;
                }
                offset += read_len.saturating_sub(pat_len.saturating_sub(1)).max(1);
            }
            return found;
        }
    }

    #[cfg(not(target_os = "none"))]
    {
        let _ = (region_start, region_size);
    }

    let canary_bytes = CANARY_VALUE.to_le_bytes();
    let mut found = 0usize;

    // Check if pattern matches canary pattern (common overflow indicator)
    let is_canary_pattern = pat_len == 4 && pat[..4] == canary_bytes;

    if is_canary_pattern {
        let table = ALLOC_TABLE.lock();
        for i in 0..MAX_ALLOC_RECORDS {
            if found >= hits.len().min(MAX_SCAN_HITS) {
                break;
            }
            if table[i].flags & 1 != 0 && (pid == 0 || table[i].pid == pid) {
                let canary_addr = table[i].addr.wrapping_add(table[i].size);
                hits[found] = canary_addr;
                found += 1;
            }
        }
    }

    found
}

/// Verifies canaries for all active allocations of a PID.
///
/// Returns the number of corrupted canaries found.
pub fn verify_canaries_for_pid(pid: u32) -> u32 {
    let mut table = ALLOC_TABLE.lock();
    let mut corrupted = 0u32;

    for i in 0..MAX_ALLOC_RECORDS {
        if table[i].flags & 1 != 0 && table[i].pid == pid {
            if table[i].canary != CANARY_VALUE {
                corrupted += 1;
                CANARY_CORRUPTIONS.fetch_add(1, Ordering::Relaxed);

                let event = MemEvent {
                    pid,
                    event_type: MemEventType::CanaryCorrupt as u8,
                    flags: 0,
                    _pad: 0,
                    addr: table[i].addr,
                    size: table[i].size,
                    timestamp: read_tsc(),
                };
                store_mem_event(event);

                // Mark canary as checked
                table[i].flags |= 2;
            }
        }
    }
    corrupted
}

/// Memory subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MemStats {
    pub total_events: u64,
    pub overflow_detections: u64,
    pub uaf_detections: u64,
    pub rate_anomalies: u64,
    pub oversized_allocs: u64,
    pub canary_corruptions: u64,
    pub active_allocations: u32,
    pub quarantined_regions: u32,
}

/// Collects memory hook statistics.
pub fn get_mem_stats() -> MemStats {
    let active = {
        let table = ALLOC_TABLE.lock();
        let mut count = 0u32;
        for i in 0..MAX_ALLOC_RECORDS {
            if table[i].flags & 1 != 0 {
                count += 1;
            }
        }
        count
    };

    MemStats {
        total_events: TOTAL_MEM_EVENTS.load(Ordering::Relaxed),
        overflow_detections: OVERFLOW_DETECTIONS.load(Ordering::Relaxed),
        uaf_detections: UAF_DETECTIONS.load(Ordering::Relaxed),
        rate_anomalies: RATE_ANOMALIES.load(Ordering::Relaxed),
        oversized_allocs: OVERSIZED_ALLOCS.load(Ordering::Relaxed),
        canary_corruptions: CANARY_CORRUPTIONS.load(Ordering::Relaxed),
        active_allocations: active,
        quarantined_regions: FREED_COUNT.load(Ordering::Relaxed),
    }
}

/// Queries recent memory events for a specific PID.
///
/// Fills `out` with matching events (most recent first) and returns
/// the number of entries written.
pub fn query_mem_events_for_pid(pid: u32, out: &mut [MemEvent]) -> usize {
    let events = MEM_EVENTS.lock();
    let head = MEM_EVENT_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_MEM_EVENTS {
        let i = (head + MAX_MEM_EVENTS - 1 - offset) % MAX_MEM_EVENTS;
        if events[i].pid == pid && events[i].timestamp != 0 {
            if written < out.len() {
                out[written] = events[i];
                written += 1;
            } else {
                break;
            }
        }
    }
    written
}

/// Resets the memory hook subsystem.
pub fn mem_hooks_init() {
    MEM_EVENT_IDX.store(0, Ordering::Release);
    ALLOC_COUNT.store(0, Ordering::Release);
    FREED_COUNT.store(0, Ordering::Release);
    TOTAL_MEM_EVENTS.store(0, Ordering::Release);
    OVERFLOW_DETECTIONS.store(0, Ordering::Release);
    UAF_DETECTIONS.store(0, Ordering::Release);
    RATE_ANOMALIES.store(0, Ordering::Release);
    OVERSIZED_ALLOCS.store(0, Ordering::Release);
    CANARY_CORRUPTIONS.store(0, Ordering::Release);
}

#[cfg(test)]
mod fix23_memory_scan_tests {
    use super::*;

    #[test]
    fn fix23_scan_buffer_finds_exact_shellcode_offsets() {
        let mut mem = [0x90u8; 64];
        mem[12..16].copy_from_slice(&[0x0F, 0x05, 0xCC, 0xCC]);
        mem[40..44].copy_from_slice(&[0x0F, 0x05, 0xCC, 0xCC]);
        let mut hits = [0u64; 8];
        let n = scan_buffer_for_pattern(0x7000_0000, &mem, &[0x0F, 0x05, 0xCC, 0xCC], &mut hits);
        assert_eq!(n, 2);
        assert_eq!(hits[0], 0x7000_000C);
        assert_eq!(hits[1], 0x7000_0028);
        assert_eq!(scan_buffer_for_pattern(0x7000_0000, &mem, &[0xDE, 0xAD], &mut hits), 0);
    }
}
