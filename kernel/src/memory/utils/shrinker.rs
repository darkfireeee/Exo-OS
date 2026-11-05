// kernel/src/memory/utils/shrinker.rs
//
// Shrinker — memory shrinkage callback table.
//
// Principle:
//   Each subsystem holding a cache (slab, dcache, inode cache, DMA
//   pool, etc.) registers a `ShrinkerFn` which, when called, frees
//   `target_pages` pages if possible and returns the count actually freed.
//
//   The low-memory path (low-memory path of OOM killer or buddy)
//   calls `run_shrinkers(target)` which traverses the table in
//   registration order and accumulates freed pages.
//
// Design:
//   • SHRINKER_TABLE: [Option<ShrinkerEntry>; MAX_SHRINKERS] = 32 slots.
//   • Shrinkers are called in registration order (priority fifo).
//   • No lock held during callback execution: table is write-once
//     (registration only at init) then read-only during operation.
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Fundamental type
// ─────────────────────────────────────────────────────────────────────────────

/// Shrinker callback type.
/// `target_pages`: number of pages to free.
/// Returns: number of pages actually freed.
pub type ShrinkerFn = fn(target_pages: u64) -> u64;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of registrable shrinkers.
pub const MAX_SHRINKERS: usize = 32;

// ─────────────────────────────────────────────────────────────────────────────
// Shrinker entry
// ─────────────────────────────────────────────────────────────────────────────

/// Entry in the shrinkers table.
pub struct ShrinkerEntry {
    /// Callback to invoke.
    pub callback: ShrinkerFn,
    /// Subsystem name (for debug/stats), max 15 chars + NUL.
    pub name: [u8; 16],
    /// Entry is active.
    pub active: bool,
    /// Priority: high-priority shrinkers are called first.
    /// Smaller value = higher priority.
    pub priority: u8,
    /// Shrinker statistics.
    pub total_freed: AtomicU64,
    pub call_count: AtomicU64,
}

impl ShrinkerEntry {
    fn new(callback: ShrinkerFn, name: &[u8], priority: u8) -> Self {
        let mut n = [0u8; 16];
        let len = name.len().min(15);
        n[..len].copy_from_slice(&name[..len]);
        Self {
            callback,
            name: n,
            active: true,
            priority,
            total_freed: AtomicU64::new(0),
            call_count: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for ShrinkerEntry {}

// ─────────────────────────────────────────────────────────────────────────────
// Table
// ─────────────────────────────────────────────────────────────────────────────

/// Identifier of a registered shrinker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShrinkerId(pub usize);

struct ShrinkerTableInner {
    entries: [Option<ShrinkerEntry>; MAX_SHRINKERS],
    count: usize,
}

impl ShrinkerTableInner {
    const fn new() -> Self {
        const NONE: Option<ShrinkerEntry> = None;
        Self {
            entries: [NONE; MAX_SHRINKERS],
            count: 0,
        }
    }
}

static SHRINKER_TABLE: Mutex<ShrinkerTableInner> = Mutex::new(ShrinkerTableInner::new());

// ─────────────────────────────────────────────────────────────────────────────
// Global statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ShrinkerStats {
    /// Total count of `run_shrinkers` calls.
    pub run_count: AtomicU64,
    /// Total pages freed across all shrinkers.
    pub pages_freed: AtomicU64,
    /// Number of times the goal could not be reached.
    pub goal_missed: AtomicU64,
    /// Number of registered shrinkers.
    pub registered: AtomicUsize,
}

impl ShrinkerStats {
    const fn new() -> Self {
        Self {
            run_count: AtomicU64::new(0),
            pages_freed: AtomicU64::new(0),
            goal_missed: AtomicU64::new(0),
            registered: AtomicUsize::new(0),
        }
    }
}

unsafe impl Sync for ShrinkerStats {}
pub static SHRINKER_STATS: ShrinkerStats = ShrinkerStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Registers a shrinker.
/// `name`: byte slice of the name (up to 15).
/// `priority`: 0 = highest priority.
/// Returns `Some(ShrinkerId)` or `None` if table full.
pub fn register_shrinker(callback: ShrinkerFn, name: &[u8], priority: u8) -> Option<ShrinkerId> {
    let mut table = SHRINKER_TABLE.lock();
    if table.count >= MAX_SHRINKERS {
        return None;
    }
    for i in 0..MAX_SHRINKERS {
        if table.entries[i].is_none() {
            table.entries[i] = Some(ShrinkerEntry::new(callback, name, priority));
            table.count += 1;
            SHRINKER_STATS.registered.fetch_add(1, Ordering::Relaxed);
            return Some(ShrinkerId(i));
        }
    }
    None
}

/// Unregisters a shrinker by its ID.
pub fn unregister_shrinker(id: ShrinkerId) -> bool {
    let mut table = SHRINKER_TABLE.lock();
    if id.0 >= MAX_SHRINKERS {
        return false;
    }
    if table.entries[id.0].is_some() {
        table.entries[id.0] = None;
        table.count -= 1;
        SHRINKER_STATS.registered.fetch_sub(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Runs shrinkers in increasing priority order until freeing
/// `target_pages` pages or exhausting all shrinkers.
///
/// Returns total number of freed pages.
pub fn run_shrinkers(target_pages: u64) -> u64 {
    SHRINKER_STATS.run_count.fetch_add(1, Ordering::Relaxed);

    // Build snapshot sorted by priority to avoid holding the lock
    // during callback execution (which may allocate).
    let mut snapshot: [Option<(u8, ShrinkerFn, usize)>; MAX_SHRINKERS] = [None; MAX_SHRINKERS];
    let mut snap_len = 0;
    {
        let table = SHRINKER_TABLE.lock();
        for i in 0..MAX_SHRINKERS {
            if let Some(ref e) = table.entries[i] {
                if e.active {
                    snapshot[snap_len] = Some((e.priority, e.callback, i));
                    snap_len += 1;
                }
            }
        }
    }
    // Sort by increasing priority.
    for i in 0..snap_len {
        for j in i + 1..snap_len {
            if let (Some(a), Some(b)) = (snapshot[i], snapshot[j]) {
                if b.0 < a.0 {
                    snapshot.swap(i, j);
                }
            }
        }
    }

    let mut total_freed = 0u64;
    let mut remaining = target_pages;

    for k in 0..snap_len {
        if let Some((_, cb, idx)) = snapshot[k] {
            if remaining == 0 {
                break;
            }
            let freed = cb(remaining);
            total_freed += freed;
            remaining = remaining.saturating_sub(freed);

            // Update entry stats.
            let table = SHRINKER_TABLE.lock();
            if let Some(ref e) = table.entries[idx] {
                e.total_freed.fetch_add(freed, Ordering::Relaxed);
                e.call_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    SHRINKER_STATS
        .pages_freed
        .fetch_add(total_freed, Ordering::Relaxed);
    if total_freed < target_pages {
        SHRINKER_STATS.goal_missed.fetch_add(1, Ordering::Relaxed);
    }
    total_freed
}

/// Runs shrinkers with a minimal target of `pages` pages; does not
/// stop until having tried everything.
pub fn shrink_all(pages: u64) -> u64 {
    run_shrinkers(pages)
}

// ─────────────────────────────────────────────────────────────────────────────
// Built-in shrinkers
// ─────────────────────────────────────────────────────────────────────────────

/// Fallback no-op shrinker (registered at low priority for testing).
fn nop_shrinker(_target: u64) -> u64 {
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Register no-op shrinker as last resort (priority 255).
    register_shrinker(nop_shrinker, b"nop", 255);
}
