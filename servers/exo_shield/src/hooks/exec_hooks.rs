//! # exec_hooks — Execution interception & chain detection
//!
//! Monitors process execution events, validates pre-exec requests against
//! blacklists and rate limits, tracks exec chains (parent→child lineage),
//! and detects rapid-fire exec patterns indicative of shellcode or exploit
//! payloads.
//!
//! ## Constraints
//! - `#![no_std]` compatible: only `core::sync::atomic` + `spin`
//! - All IPC-facing types are `#[repr(C)]`
//! - Static arrays only — no Vec, String, Box, HashMap

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of distinct PIDs tracked for exec frequency.
const MAX_FREQ_ENTRIES: usize = 128;

/// Maximum number of exec chain entries (parent→child lineage).
const MAX_CHAIN_ENTRIES: usize = 256;

/// Maximum number of blacklisted path hashes.
const MAX_BLACKLIST_ENTRIES: usize = 64;

/// Maximum number of recent exec events stored.
const MAX_EXEC_EVENTS: usize = 512;

/// Exec rate limit: maximum execs per PID within the tracking window.
const EXEC_RATE_LIMIT: u32 = 32;

/// Tracking window in TSC ticks (~1 second at 3 GHz).
const TRACKING_WINDOW_TSC: u64 = 3_000_000_000;

/// Maximum exec chain depth before flagging as anomalous.
const MAX_CHAIN_DEPTH: u32 = 8;

// ── FNV-1a hash (64-bit) ─────────────────────────────────────────────────────

/// Simple FNV-1a hash for path fingerprinting. No heap, no std.
#[inline]
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

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

/// Action returned by the pre-exec validation hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExecAction {
    /// Allow the exec to proceed.
    Allow = 0,
    /// Deny the exec — policy violation or rate limit exceeded.
    Deny = 1,
    /// Allow but flag for enhanced monitoring.
    Monitor = 2,
    /// Kill the process attempting exec.
    Kill = 3,
}

impl ExecAction {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Allow,
            1 => Self::Deny,
            2 => Self::Monitor,
            3 => Self::Kill,
            _ => Self::Allow,
        }
    }
}

/// Exec event recorded for each exec(2) attempt.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExecEvent {
    /// PID of the process calling exec.
    pub pid: u32,
    /// Parent PID at exec time.
    pub ppid: u32,
    /// FNV-1a hash of the executed path (up to 256 bytes).
    pub path_hash: u64,
    /// UID of the caller.
    pub uid: u32,
    /// Flags: bit 0 = setuid, bit 1 = setgid, bit 2 = shell, bit 3 = script.
    pub flags: u8,
    /// Result of the hook: 0=allow, 1=deny, 2=monitor, 3=kill.
    pub action: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// TSC timestamp of the event.
    pub timestamp: u64,
}

impl Default for ExecEvent {
    fn default() -> Self {
        Self {
            pid: 0,
            ppid: 0,
            path_hash: 0,
            uid: 0,
            flags: 0,
            action: ExecAction::Allow as u8,
            _pad: [0; 2],
            timestamp: 0,
        }
    }
}

/// Exec frequency tracking entry (per PID).
struct ExecFreqEntry {
    pid: AtomicU32,
    count: AtomicU32,
    window_start: AtomicU64,
}

impl ExecFreqEntry {
    const fn new() -> Self {
        Self {
            pid: AtomicU32::new(0),
            count: AtomicU32::new(0),
            window_start: AtomicU64::new(0),
        }
    }
}

/// Exec chain entry tracking parent→child lineage.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExecChainEntry {
    /// Child PID.
    pub child_pid: u32,
    /// Parent PID.
    pub parent_pid: u32,
    /// Hash of the child executable path.
    pub child_path_hash: u64,
    /// Hash of the parent executable path.
    pub parent_path_hash: u64,
    /// Depth in the chain (0 = root process).
    pub depth: u32,
    /// TSC timestamp of chain creation.
    pub timestamp: u64,
}

impl Default for ExecChainEntry {
    fn default() -> Self {
        Self {
            child_pid: 0,
            parent_pid: 0,
            child_path_hash: 0,
            parent_path_hash: 0,
            depth: 0,
            timestamp: 0,
        }
    }
}

/// Blacklist entry — stores a hash of a forbidden path.
struct BlacklistEntry {
    path_hash: AtomicU64,
    flags: AtomicU8,
}

impl BlacklistEntry {
    const fn new() -> Self {
        Self {
            path_hash: AtomicU64::new(0),
            flags: AtomicU8::new(0),
        }
    }
}

// ── Static storage ────────────────────────────────────────────────────────────

/// Ring buffer of recent exec events.
static EXEC_EVENTS: Mutex<[ExecEvent; MAX_EXEC_EVENTS]> = Mutex::new(
    [ExecEvent {
        pid: 0,
        ppid: 0,
        path_hash: 0,
        uid: 0,
        flags: 0,
        action: 0,
        _pad: [0; 2],
        timestamp: 0,
    }; MAX_EXEC_EVENTS],
);

/// Write index into the exec event ring buffer.
static EXEC_EVENT_IDX: AtomicU32 = AtomicU32::new(0);

/// Exec frequency table.
static EXEC_FREQ_TABLE: Mutex<[ExecFreqEntry; MAX_FREQ_ENTRIES]> = Mutex::new(
    [ExecFreqEntry::new(); MAX_FREQ_ENTRIES],
);

/// Exec chain table.
static EXEC_CHAIN_TABLE: Mutex<[ExecChainEntry; MAX_CHAIN_ENTRIES]> = Mutex::new(
    [ExecChainEntry::default(); MAX_CHAIN_ENTRIES],
);

/// Write index for the exec chain table.
static EXEC_CHAIN_IDX: AtomicU32 = AtomicU32::new(0);

/// Blacklist of forbidden path hashes.
static BLACKLIST: Mutex<[BlacklistEntry; MAX_BLACKLIST_ENTRIES]> = Mutex::new(
    [BlacklistEntry::new(); MAX_BLACKLIST_ENTRIES],
);

/// Number of active blacklist entries.
static BLACKLIST_COUNT: AtomicU32 = AtomicU32::new(0);

/// Statistics counters.
static TOTAL_EXECS: AtomicU64 = AtomicU64::new(0);
static DENIED_EXECS: AtomicU64 = AtomicU64::new(0);
static RATE_LIMITED: AtomicU64 = AtomicU64::new(0);
static CHAIN_ANOMALIES: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Check whether a path hash is on the blacklist.
fn is_blacklisted(path_hash: u64) -> bool {
    let bl = BLACKLIST.lock();
    let count = BLACKLIST_COUNT.load(Ordering::Acquire) as usize;
    for i in 0..count.min(MAX_BLACKLIST_ENTRIES) {
        if bl[i].path_hash.load(Ordering::Acquire) == path_hash {
            let flags = bl[i].flags.load(Ordering::Acquire);
            // flags bit 0 = active
            if flags & 1 != 0 {
                return true;
            }
        }
    }
    false
}

/// Increment the exec frequency counter for a PID.
/// Returns the current count within the tracking window.
fn bump_exec_freq(pid: u32) -> u32 {
    let mut table = EXEC_FREQ_TABLE.lock();
    let now = read_tsc();

    // Search for existing entry
    for i in 0..MAX_FREQ_ENTRIES {
        let entry_pid = table[i].pid.load(Ordering::Acquire);
        if entry_pid == pid {
            let start = table[i].window_start.load(Ordering::Acquire);
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                // Window expired — reset
                table[i].count.store(1, Ordering::Release);
                table[i].window_start.store(now, Ordering::Release);
                return 1;
            }
            let new_count = table[i].count.fetch_add(1, Ordering::AcqRel) + 1;
            return new_count;
        }
    }

    // No entry found — find an empty or expired slot
    for i in 0..MAX_FREQ_ENTRIES {
        let entry_pid = table[i].pid.load(Ordering::Acquire);
        if entry_pid == 0 {
            table[i].pid.store(pid, Ordering::Release);
            table[i].count.store(1, Ordering::Release);
            table[i].window_start.store(now, Ordering::Release);
            return 1;
        }
        // Check if this entry's window has expired — reclaim it
        let start = table[i].window_start.load(Ordering::Acquire);
        if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
            table[i].pid.store(pid, Ordering::Release);
            table[i].count.store(1, Ordering::Release);
            table[i].window_start.store(now, Ordering::Release);
            return 1;
        }
    }

    // Table full — overwrite the entry with the oldest window
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..MAX_FREQ_ENTRIES {
        let start = table[i].window_start.load(Ordering::Acquire);
        if start < oldest_tsc {
            oldest_tsc = start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].pid.store(pid, Ordering::Release);
    table[oldest_idx].count.store(1, Ordering::Release);
    table[oldest_idx].window_start.store(now, Ordering::Release);
    1
}

/// Compute the exec chain depth for a given PID.
fn compute_chain_depth(pid: u32) -> u32 {
    let table = EXEC_CHAIN_TABLE.lock();
    let count = EXEC_CHAIN_IDX.load(Ordering::Acquire) as usize;
    let mut current_pid = pid;
    let mut depth = 0u32;

    // Walk the chain backwards: child→parent
    for _ in 0..MAX_CHAIN_DEPTH {
        let mut found = false;
        for i in 0..count.min(MAX_CHAIN_ENTRIES) {
            if table[i].child_pid == current_pid {
                depth = depth.saturating_add(1);
                current_pid = table[i].parent_pid;
                if current_pid == 0 || current_pid == 1 {
                    return depth;
                }
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }
    depth
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Validates an exec request before it is carried out.
///
/// Checks:
/// 1. Path hash against the blacklist → `Deny` or `Kill`
/// 2. Exec frequency for the calling PID → `Deny` if rate exceeded
/// 3. Exec chain depth → `Monitor` if depth exceeds threshold
/// 4. Default → `Allow`
///
/// Returns the action the kernel should take.
pub fn pre_exec_validate(pid: u32, ppid: u32, path: &[u8], uid: u32, flags: u8) -> ExecAction {
    let path_hash = fnv1a_hash(path);
    TOTAL_EXECS.fetch_add(1, Ordering::Relaxed);

    // 1. Blacklist check
    if is_blacklisted(path_hash) {
        DENIED_EXECS.fetch_add(1, Ordering::Relaxed);
        // setuid/setgid blacklisted binary → kill
        if flags & 0x03 != 0 {
            return ExecAction::Kill;
        }
        return ExecAction::Deny;
    }

    // 2. Rate limit check
    let freq = bump_exec_freq(pid);
    if freq > EXEC_RATE_LIMIT {
        RATE_LIMITED.fetch_add(1, Ordering::Relaxed);
        DENIED_EXECS.fetch_add(1, Ordering::Relaxed);
        return ExecAction::Deny;
    }

    // 3. Chain depth check
    let depth = compute_chain_depth(pid);
    if depth >= MAX_CHAIN_DEPTH {
        CHAIN_ANOMALIES.fetch_add(1, Ordering::Relaxed);
        return ExecAction::Monitor;
    }

    // 4. Shell/script exec by privileged user → monitor
    if uid == 0 && (flags & 0x0C != 0) {
        return ExecAction::Monitor;
    }

    ExecAction::Allow
}

/// Records the outcome of an exec and performs post-exec monitoring.
///
/// This is called after the kernel has processed the exec. It stores
/// the event in the ring buffer and records the exec chain link.
pub fn post_exec_monitor(
    pid: u32,
    ppid: u32,
    path: &[u8],
    uid: u32,
    flags: u8,
    action: ExecAction,
) {
    let path_hash = fnv1a_hash(path);
    let now = read_tsc();

    // Record event in ring buffer
    let event = ExecEvent {
        pid,
        ppid,
        path_hash,
        uid,
        flags,
        action: action as u8,
        _pad: [0; 2],
        timestamp: now,
    };

    {
        let mut events = EXEC_EVENTS.lock();
        let idx = EXEC_EVENT_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_EXEC_EVENTS;
        events[idx] = event;
    }

    // Record chain link if action was Allow or Monitor
    if action == ExecAction::Allow || action == ExecAction::Monitor {
        let parent_hash = {
            let events = EXEC_EVENTS.lock();
            let mut found = 0u64;
            // Search recent events for the parent's path hash
            let base = EXEC_EVENT_IDX.load(Ordering::Acquire) as usize;
            for offset in 0..MAX_EXEC_EVENTS.min(64) {
                let i = (base + MAX_EXEC_EVENTS - 1 - offset) % MAX_EXEC_EVENTS;
                if events[i].pid == ppid {
                    found = events[i].path_hash;
                    break;
                }
            }
            found
        };

        let depth = compute_chain_depth(pid);
        let chain_entry = ExecChainEntry {
            child_pid: pid,
            parent_pid: ppid,
            child_path_hash: path_hash,
            parent_path_hash: parent_hash,
            depth,
            timestamp: now,
        };

        let mut chains = EXEC_CHAIN_TABLE.lock();
        let idx = EXEC_CHAIN_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_CHAIN_ENTRIES;
        chains[idx] = chain_entry;
    }
}

/// Checks the exec chain for the given PID and returns the chain depth
/// and whether it is considered anomalous.
///
/// Returns `(depth, is_anomalous)`.
pub fn check_exec_chain(pid: u32) -> (u32, bool) {
    let depth = compute_chain_depth(pid);
    (depth, depth >= MAX_CHAIN_DEPTH)
}

/// Records an exec event directly into the ring buffer.
/// Useful for replaying events from the kernel's audit log.
pub fn record_exec_event(event: ExecEvent) {
    let mut events = EXEC_EVENTS.lock();
    let idx = EXEC_EVENT_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_EXEC_EVENTS;
    events[idx] = event;
}

/// Adds a path to the exec blacklist.
///
/// The path is stored as an FNV-1a hash so the blacklist itself does not
/// reveal path strings.
///
/// Returns `true` if the entry was successfully added.
pub fn add_blacklist_path(path: &[u8]) -> bool {
    let hash = fnv1a_hash(path);
    let mut bl = BLACKLIST.lock();
    let count = BLACKLIST_COUNT.load(Ordering::Acquire) as usize;

    // Check for duplicate
    for i in 0..count.min(MAX_BLACKLIST_ENTRIES) {
        if bl[i].path_hash.load(Ordering::Acquire) == hash {
            // Already present — just ensure active flag is set
            bl[i].flags.store(1, Ordering::Release);
            return true;
        }
    }

    // Add new entry
    if count < MAX_BLACKLIST_ENTRIES {
        bl[count].path_hash.store(hash, Ordering::Release);
        bl[count].flags.store(1, Ordering::Release);
        BLACKLIST_COUNT.fetch_add(1, Ordering::Release);
        return true;
    }
    false
}

/// Removes a path from the exec blacklist.
///
/// Returns `true` if the entry was found and deactivated.
pub fn remove_blacklist_path(path: &[u8]) -> bool {
    let hash = fnv1a_hash(path);
    let bl = BLACKLIST.lock();
    let count = BLACKLIST_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_BLACKLIST_ENTRIES) {
        if bl[i].path_hash.load(Ordering::Acquire) == hash {
            bl[i].flags.store(0, Ordering::Release);
            return true;
        }
    }
    false
}

/// Returns the chain of exec entries for a given PID, walking from
/// child back to the root. Fills the provided buffer and returns the
/// number of entries written.
pub fn get_exec_chain_for_pid(pid: u32, out: &mut [ExecChainEntry]) -> usize {
    let table = EXEC_CHAIN_TABLE.lock();
    let count = EXEC_CHAIN_IDX.load(Ordering::Acquire) as usize;
    let mut current_pid = pid;
    let mut written = 0usize;

    for _ in 0..out.len().min(MAX_CHAIN_DEPTH as usize) {
        let mut found = false;
        for i in 0..count.min(MAX_CHAIN_ENTRIES) {
            if table[i].child_pid == current_pid {
                if written < out.len() {
                    out[written] = table[i];
                    written += 1;
                }
                current_pid = table[i].parent_pid;
                if current_pid == 0 || current_pid == 1 {
                    return written;
                }
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }
    written
}

/// Exec subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ExecStats {
    pub total_execs: u64,
    pub denied_execs: u64,
    pub rate_limited: u64,
    pub chain_anomalies: u64,
    pub blacklist_count: u32,
    pub active_freq_entries: u32,
}

/// Collects exec hook statistics.
pub fn get_exec_stats() -> ExecStats {
    let freq_count = {
        let table = EXEC_FREQ_TABLE.lock();
        let now = read_tsc();
        let mut count = 0u32;
        for i in 0..MAX_FREQ_ENTRIES {
            let pid = table[i].pid.load(Ordering::Acquire);
            if pid != 0 {
                let start = table[i].window_start.load(Ordering::Acquire);
                if now.wrapping_sub(start) <= TRACKING_WINDOW_TSC {
                    count += 1;
                }
            }
        }
        count
    };

    ExecStats {
        total_execs: TOTAL_EXECS.load(Ordering::Relaxed),
        denied_execs: DENIED_EXECS.load(Ordering::Relaxed),
        rate_limited: RATE_LIMITED.load(Ordering::Relaxed),
        chain_anomalies: CHAIN_ANOMALIES.load(Ordering::Relaxed),
        blacklist_count: BLACKLIST_COUNT.load(Ordering::Relaxed),
        active_freq_entries: freq_count,
    }
}

/// Queries recent exec events for a specific PID.
///
/// Fills `out` with matching events (most recent first) and returns
/// the number of entries written.
pub fn query_exec_events_for_pid(pid: u32, out: &mut [ExecEvent]) -> usize {
    let events = EXEC_EVENTS.lock();
    let head = EXEC_EVENT_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_EXEC_EVENTS {
        let i = (head + MAX_EXEC_EVENTS - 1 - offset) % MAX_EXEC_EVENTS;
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

/// Resets the exec hook subsystem. Used during exo_shield initialization.
pub fn exec_hooks_init() {
    EXEC_EVENT_IDX.store(0, Ordering::Release);
    EXEC_CHAIN_IDX.store(0, Ordering::Release);
    BLACKLIST_COUNT.store(0, Ordering::Release);
    TOTAL_EXECS.store(0, Ordering::Release);
    DENIED_EXECS.store(0, Ordering::Release);
    RATE_LIMITED.store(0, Ordering::Release);
    CHAIN_ANOMALIES.store(0, Ordering::Release);

    {
        let mut freq = EXEC_FREQ_TABLE.lock();
        for entry in freq.iter_mut() {
            entry.pid.store(0, Ordering::Release);
            entry.count.store(0, Ordering::Release);
            entry.window_start.store(0, Ordering::Release);
        }
    }
}
