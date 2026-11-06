// ipc/sync/event.rs — Binary IPC event and counting event for Exo-OS
//
// This module implements two lightweight synchronization primitives:
//
// 1. `IpcEvent` — binary event (set/clear/wait) based on AtomicU32.
//    A single state bit suffices. Supports "auto-reset" mode (the flag is
//    cleared as soon as a waiter is woken) and "manual-reset" mode
//    (remains set until explicit clear()).
//
// 2. `IpcCountingEvent` — counting event (lightweight semaphore):
//    each `signal()` increments a counter; `wait()` decrements it.
//    If the counter is zero, the thread spin-waits.
//
// RULE EVENT-01: no Vec, no Box. Tables are static.
// RULE EVENT-02: spin-wait includes a timeout counter.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// IpcEvent — binary event
// ---------------------------------------------------------------------------

/// Event reset mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EventMode {
    /// The event is cleared automatically upon first wakeup
    AutoReset = 0,
    /// The event remains set until explicit `clear()`
    ManualReset = 1,
}

/// Internal states of IpcEvent
mod event_state {
    pub const CLEAR: u32 = 0;
    pub const SET: u32 = 1;
}

/// Binary IPC event.  
///
/// Thread-safe, no_std. Spin-wait is bounded by `spin_max` iterations.
#[repr(C, align(64))]
pub struct IpcEvent {
    /// Opaque name (debug)
    pub id: u32,
    /// Current state
    state: AtomicU32,
    /// Auto-reset vs manual-reset mode
    mode: AtomicU32,
    /// Current number of waiters
    waiter_count: AtomicU32,
    /// Active operations counter (anti-UAF on concurrent destruction)
    pub active_ops: AtomicU32,
    /// Destruction/closure marker
    closed: AtomicBool,
    /// Statistics
    pub total_sets: AtomicU64,
    pub total_clears: AtomicU64,
    pub total_waits: AtomicU64,
    pub total_timeouts: AtomicU64,
    _pad: [u8; 16],
}

// SAFETY: all internal fields are atomic
unsafe impl Sync for IpcEvent {}
unsafe impl Send for IpcEvent {}

impl IpcEvent {
    pub const fn new(id: u32, mode: EventMode) -> Self {
        Self {
            id,
            state: AtomicU32::new(event_state::CLEAR),
            mode: AtomicU32::new(mode as u32),
            waiter_count: AtomicU32::new(0),
            active_ops: AtomicU32::new(0),
            closed: AtomicBool::new(false),
            total_sets: AtomicU64::new(0),
            total_clears: AtomicU64::new(0),
            total_waits: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    /// Closes the event and signals destruction to waiters.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Sets the event (wakes waiters if AUTO_RESET or all if MANUAL).
    pub fn set(&self) {
        self.state.store(event_state::SET, Ordering::Release);
        self.total_sets.fetch_add(1, Ordering::Relaxed);
    }

    /// Manually clears the event.
    pub fn clear(&self) {
        self.state.store(event_state::CLEAR, Ordering::Release);
        self.total_clears.fetch_add(1, Ordering::Relaxed);
    }

    /// Checks the state without waiting.
    pub fn is_set(&self) -> bool {
        self.state.load(Ordering::Acquire) == event_state::SET
    }

    /// Waits for the event to be set.
    ///
    /// - `spin_max=0` -> infinite wait
    /// - In AUTO_RESET mode: atomically clears the event before returning
    ///
    /// Returns `Ok(())` if event was detected,  
    /// `Err(IpcError::Timeout)` if timeout expired.
    pub fn wait(&self, spin_max: u64) -> Result<(), IpcError> {
        self.waiter_count.fetch_add(1, Ordering::Relaxed);
        self.total_waits.fetch_add(1, Ordering::Relaxed);

        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                // SAFETY: cooperative_reschedule temporarily yields if scheduler is active
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }

            if self.closed.load(Ordering::Acquire) {
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                return Err(IpcError::ChannelClosed);
            }

            if self.state.load(Ordering::Acquire) == event_state::SET {
                let mode = self.mode.load(Ordering::Relaxed);
                if mode == EventMode::AutoReset as u32 {
                    // CAS to acquire event in mutual exclusion
                    match self.state.compare_exchange(
                        event_state::SET,
                        event_state::CLEAR,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => {
                            self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                            return Ok(());
                        }
                        Err(_) => {
                            // Another thread acquired the event, keep waiting
                            continue;
                        }
                    }
                } else {
                    // ManualReset: everyone is woken
                    self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                    return Ok(());
                }
            }

            if spins >= limit {
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Waits and clears in a single operation (atomic for AUTO_RESET).
    pub fn wait_and_clear(&self, spin_max: u64) -> Result<(), IpcError> {
        self.wait(spin_max)
    }

    /// Statistics snapshot
    pub fn snapshot(&self) -> IpcEventStats {
        IpcEventStats {
            id: self.id,
            is_set: self.is_set(),
            waiter_count: self.waiter_count.load(Ordering::Relaxed),
            total_sets: self.total_sets.load(Ordering::Relaxed),
            total_clears: self.total_clears.load(Ordering::Relaxed),
            total_waits: self.total_waits.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcEventStats {
    pub id: u32,
    pub is_set: bool,
    pub waiter_count: u32,
    pub total_sets: u64,
    pub total_clears: u64,
    pub total_waits: u64,
    pub total_timeouts: u64,
}

// ---------------------------------------------------------------------------
// IpcCountingEvent — lightweight semaphore based on an atomic counter
// ---------------------------------------------------------------------------

/// Maximum counter value (overflow protection)
pub const MAX_EVENT_COUNT: u32 = 65535;

/// Lightweight IPC semaphore.
///
/// `signal()` -> increments counter.  
/// `wait()` -> decrements if counter > 0, otherwise spin-wait.
#[repr(C, align(64))]
pub struct IpcCountingEvent {
    pub id: u32,
    count: AtomicU32,
    max_count: AtomicU32,
    pub total_signals: AtomicU64,
    pub total_waits: AtomicU64,
    pub total_timeouts: AtomicU64,
    _pad: [u8; 16],
}

unsafe impl Sync for IpcCountingEvent {}
unsafe impl Send for IpcCountingEvent {}

impl IpcCountingEvent {
    pub const fn new(id: u32, initial: u32, max_count: u32) -> Self {
        let cap = if max_count > MAX_EVENT_COUNT {
            MAX_EVENT_COUNT
        } else {
            max_count
        };
        let init = if initial > cap { cap } else { initial };
        Self {
            id,
            count: AtomicU32::new(init),
            max_count: AtomicU32::new(cap),
            total_signals: AtomicU64::new(0),
            total_waits: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    /// Increments the counter (signal). Returns immediately.
    /// Returns `Err(IpcError::Invalid)` if already at maximum.
    pub fn signal(&self) -> Result<(), IpcError> {
        let max = self.max_count.load(Ordering::Relaxed);
        loop {
            let old = self.count.load(Ordering::Relaxed);
            if old >= max {
                return Err(IpcError::Invalid);
            }
            match self.count.compare_exchange_weak(
                old,
                old + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.total_signals.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    /// Waits for counter to be > 0 then decrements.
    pub fn wait(&self, spin_max: u64) -> Result<(), IpcError> {
        self.total_waits.fetch_add(1, Ordering::Relaxed);
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                // SAFETY: cooperative_reschedule temporarily yields if scheduler is active
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }

            let old = self.count.load(Ordering::Acquire);
            if old > 0 {
                match self.count.compare_exchange_weak(
                    old,
                    old - 1,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Ok(()),
                    Err(_) => continue,
                }
            }

            if spins >= limit {
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Attempts to decrement without waiting.
    pub fn try_wait(&self) -> bool {
        loop {
            let old = self.count.load(Ordering::Acquire);
            if old == 0 {
                return false;
            }
            if self
                .count
                .compare_exchange_weak(old, old - 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Current counter value.
    pub fn count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    /// Resets counter to `initial`.
    pub fn reset(&self, initial: u32) {
        let max = self.max_count.load(Ordering::Relaxed);
        let v = if initial > max { max } else { initial };
        self.count.store(v, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Global IPC event table
// ---------------------------------------------------------------------------

/// Maximum number of IpcEvents in global table
pub const MAX_IPC_EVENTS: usize = 128;

/// Slot in global table
#[repr(C)]
struct EventSlot {
    event: MaybeUninit<IpcEvent>,
    occupied: AtomicBool,
}

impl EventSlot {
    const fn empty() -> Self {
        Self {
            event: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

/// Global table of IpcEvents
struct IpcEventTable {
    slots: [EventSlot; MAX_IPC_EVENTS],
    count: AtomicU32,
}

// SAFETY: access protected by CAS on `occupied`
unsafe impl Sync for IpcEventTable {}

impl IpcEventTable {
    const fn new() -> Self {
        const EMPTY: EventSlot = EventSlot::empty();
        Self {
            slots: [EMPTY; MAX_IPC_EVENTS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32, mode: EventMode) -> Option<usize> {
        for i in 0..MAX_IPC_EVENTS {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i]
                    .occupied
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    let ptr = self.slots[i].event.as_ptr() as *mut IpcEvent;
                    // SAFETY: CAS AcqRel guarantees exclusivity; MaybeUninit<IpcEvent> write-once.
                    unsafe {
                        ptr.write(IpcEvent::new(id, mode));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&IpcEvent> {
        if idx >= MAX_IPC_EVENTS {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: slot initialized (occupied=true)
        Some(unsafe { &*self.slots[idx].event.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_IPC_EVENTS {
            return false;
        }
        if self.slots[idx]
            .occupied
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.count.fetch_sub(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

// Global static table
static IPC_EVENT_TABLE: IpcEventTable = IpcEventTable::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

struct ActiveOpGuard<'a>(&'a AtomicU32);

impl<'a> Drop for ActiveOpGuard<'a> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// Creates an IPC event in the global table.
/// Returns the index (handle) of the event.
pub fn event_create(id: u32, mode: EventMode) -> Option<usize> {
    IPC_EVENT_TABLE.alloc(id, mode)
}

/// Sets event `idx`.
pub fn event_set(idx: usize) -> Result<(), IpcError> {
    let e = IPC_EVENT_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?;
    e.active_ops.fetch_add(1, Ordering::AcqRel);
    let _guard = ActiveOpGuard(&e.active_ops);
    e.set();
    Ok(())
}

/// Clears event `idx`.
pub fn event_clear(idx: usize) -> Result<(), IpcError> {
    let e = IPC_EVENT_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?;
    e.active_ops.fetch_add(1, Ordering::AcqRel);
    let _guard = ActiveOpGuard(&e.active_ops);
    e.clear();
    Ok(())
}

/// Checks the event state without waiting.
pub fn event_is_set(idx: usize) -> Option<bool> {
    IPC_EVENT_TABLE.get(idx).map(|e| e.is_set())
}

/// Waits for event `idx` with spin-max.
pub fn event_wait(idx: usize, spin_max: u64) -> Result<(), IpcError> {
    let e = IPC_EVENT_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?;
    e.active_ops.fetch_add(1, Ordering::AcqRel);
    let _guard = ActiveOpGuard(&e.active_ops);
    e.wait(spin_max)
}

/// Destroys an event (frees its slot).
pub fn event_destroy(idx: usize) -> bool {
    if let Some(event) = IPC_EVENT_TABLE.get(idx) {
        event.close();
        let mut spins = 0u64;
        while event.active_ops.load(Ordering::Acquire) > 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                // SAFETY: cooperative_reschedule temporarily yields if scheduler is active
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
        }
    }
    IPC_EVENT_TABLE.free(idx)
}

/// Counter of created events.
pub fn event_count() -> u32 {
    IPC_EVENT_TABLE.count()
}

/// Retrieves statistics snapshot for an event.
pub fn event_stats(idx: usize) -> Option<IpcEventStats> {
    IPC_EVENT_TABLE.get(idx).map(|e| e.snapshot())
}
