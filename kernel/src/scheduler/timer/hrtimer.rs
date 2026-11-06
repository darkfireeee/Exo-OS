// kernel/src/scheduler/timer/hrtimer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// HRTimer — high-resolution timers (ns) based on sorted list
// ═══════════════════════════════════════════════════════════════════════════════
//
// Supports up to MAX_HRTIMERS concurrent timers per CPU.
// List is sorted by increasing expiry (no red-black tree in
// no_alloc — fixed array sorted by insertion).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::timer::clock::monotonic_ns;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const MAX_HRTIMERS: usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Timer structure
// ─────────────────────────────────────────────────────────────────────────────

type HrTimerCallback = unsafe fn(u32, u64);

#[derive(Clone, Copy)]
struct HrTimerEntry {
    expiry_ns: u64,
    id: u32,
    data: u64,
    callback: Option<HrTimerCallback>,
}

impl HrTimerEntry {
    const fn empty() -> Self {
        Self {
            expiry_ns: u64::MAX,
            id: 0,
            data: 0,
            callback: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU timer list
// ─────────────────────────────────────────────────────────────────────────────

struct HrTimerList {
    entries: [HrTimerEntry; MAX_HRTIMERS],
    count: usize,
    next_id: u32,
}

impl HrTimerList {
    const fn new() -> Self {
        Self {
            entries: [HrTimerEntry::empty(); MAX_HRTIMERS],
            count: 0,
            next_id: 1,
        }
    }

    /// Inserts a timer sorted by expiry. Returns its ID or 0 if full.
    fn insert(&mut self, expiry_ns: u64, data: u64, cb: HrTimerCallback) -> u32 {
        if self.count >= MAX_HRTIMERS {
            return 0;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);

        let entry = HrTimerEntry {
            expiry_ns,
            id,
            data,
            callback: Some(cb),
        };
        // Insertion sorted by expiry_ns.
        let mut pos = self.count;
        for i in 0..self.count {
            if self.entries[i].expiry_ns > expiry_ns {
                pos = i;
                break;
            }
        }
        // Shift right.
        let mut j = self.count;
        while j > pos {
            self.entries[j] = self.entries[j - 1];
            j -= 1;
        }
        self.entries[pos] = entry;
        self.count += 1;
        id
    }

    /// Cancels timer by ID.
    fn cancel(&mut self, id: u32) -> bool {
        for i in 0..self.count {
            if self.entries[i].id == id {
                let mut j = i;
                while j + 1 < self.count {
                    self.entries[j] = self.entries[j + 1];
                    j += 1;
                }
                self.entries[self.count - 1] = HrTimerEntry::empty();
                self.count -= 1;
                return true;
            }
        }
        false
    }

    /// Fires all expired timers. Returns count.
    unsafe fn fire_expired(&mut self) -> usize {
        let now = monotonic_ns();
        let mut fired = 0usize;
        while self.count > 0 && self.entries[0].expiry_ns <= now {
            let e = self.entries[0];
            // Remove first to prevent reentrancy.
            let mut j = 0;
            while j + 1 < self.count {
                self.entries[j] = self.entries[j + 1];
                j += 1;
            }
            self.entries[self.count - 1] = HrTimerEntry::empty();
            self.count -= 1;
            if let Some(cb) = e.callback {
                cb(e.id, e.data);
            }
            fired += 1;
        }
        fired
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global instances (one per CPU — indexed by CPU ID)
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::smp::topology::MAX_CPUS;
use core::mem::MaybeUninit;

static mut HR_LISTS: [MaybeUninit<HrTimerList>; MAX_CPUS] =
    [const { MaybeUninit::uninit() }; MAX_CPUS];

pub static HRTIMER_FIRED: AtomicU64 = AtomicU64::new(0);
pub static HRTIMER_CANCELLED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes hrtimer lists for `nr_cpus` CPUs.
///
/// # Safety
/// Called once from `scheduler::init()`.
pub unsafe fn init(nr_cpus: usize) {
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        HR_LISTS[cpu].write(HrTimerList::new());
    }
}

/// Arms a timer on CPU `cpu`, firing `delay_ns` after now.
///
/// # Safety
/// Requires preemption disabled; CPU must be initialized.
pub unsafe fn arm(cpu: usize, delay_ns: u64, data: u64, cb: HrTimerCallback) -> u32 {
    let _irq = crate::scheduler::core::preempt::IrqGuard::new();
    let expiry = monotonic_ns().saturating_add(delay_ns);
    HR_LISTS[cpu].assume_init_mut().insert(expiry, data, cb)
}

/// Cancels timer `id` on CPU `cpu`.
///
/// # Safety
/// Requires preemption disabled.
pub unsafe fn cancel(cpu: usize, id: u32) -> bool {
    let _irq = crate::scheduler::core::preempt::IrqGuard::new();
    let r = HR_LISTS[cpu].assume_init_mut().cancel(id);
    if r {
        HRTIMER_CANCELLED.fetch_add(1, Ordering::Relaxed);
    }
    r
}

/// Fires expired timers on CPU `cpu`.
/// Called from tick handler after each tick.
///
/// # Safety
/// Requires preemption disabled.
pub unsafe fn fire_expired(cpu: usize) -> usize {
    let _irq = crate::scheduler::core::preempt::IrqGuard::new();
    let fired = HR_LISTS[cpu].assume_init_mut().fire_expired();
    HRTIMER_FIRED.fetch_add(fired as u64, Ordering::Relaxed);
    fired
}
