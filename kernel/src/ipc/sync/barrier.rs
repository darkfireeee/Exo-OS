// ipc/sync/barrier.rs — IPC synchronization barrier for Exo-OS
//
// This module implements a cyclic barrier (reusable barrier) for N threads.
// Each participant calls `arrive_and_wait()`; all are blocked until
// all N participants have arrived, then all are released simultaneously.
//
// The barrier is cyclic: after release, the counter is reset to zero
// automatically. A phase number (generation) protects against spurious
// wakeups and races from one cycle to the next.
//
// RULE BARRIER-01: no dynamic allocation, global static table.
// RULE BARRIER-02: spin-wait bounded by configurable timeout.
// RULE BARRIER-03: AtomicU32 generation guarantees cyclic correctness.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// Main IPC barrier
// ---------------------------------------------------------------------------

/// Maximum number of participants per barrier
pub const MAX_BARRIER_PARTIES: usize = 256;

/// Maximum number of barriers in global table
pub const MAX_IPC_BARRIERS: usize = 64;

/// Phase states of the barrier
mod phase_state {
    pub const WAITING: u32 = 0;
    #[allow(dead_code)]
    pub const RELEASING: u32 = 1;
}

/// Result of an `arrive_and_wait()` call
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierResult {
    /// This thread was the last to arrive (leader thread)
    Leader,
    /// This thread was released normally
    Follower,
}

/// Cyclic IPC synchronization barrier (reusable).
///
/// Supports up to `MAX_BARRIER_PARTIES` participants.  
/// Reusable after each cycle thanks to generation counter.
#[repr(C, align(64))]
pub struct IpcBarrier {
    /// Opaque identifier
    pub id: u32,
    /// Number of participants required to cross the barrier
    parties: AtomicU32,
    /// Arrival counter for current cycle
    arrived: AtomicU32,
    /// Generation number (current phase)
    generation: AtomicU32,
    /// Barrier phase
    phase: AtomicU32,
    /// Number of completed cycles
    pub cycles: AtomicU64,
    /// Total number of arrivals
    pub total_arrives: AtomicU64,
    /// Number of timeouts
    pub total_timeouts: AtomicU64,
    /// Active barrier (not destroyed)
    pub active: AtomicBool,
    _pad: [u8; 15],
}

// SAFETY: all fields are atomic or primitive
unsafe impl Sync for IpcBarrier {}
unsafe impl Send for IpcBarrier {}

impl IpcBarrier {
    pub const fn new(id: u32, parties: u32) -> Self {
        Self {
            id,
            parties: AtomicU32::new(parties),
            arrived: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            phase: AtomicU32::new(phase_state::WAITING),
            cycles: AtomicU64::new(0),
            total_arrives: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            active: AtomicBool::new(true),
            _pad: [0u8; 15],
        }
    }

    /// Returns number of participants
    pub fn parties(&self) -> u32 {
        self.parties.load(Ordering::Relaxed)
    }

    /// Returns number of participants that have already arrived in this cycle
    pub fn arrived_count(&self) -> u32 {
        self.arrived.load(Ordering::Relaxed)
    }

    /// Returns current generation (phase)
    pub fn current_generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // arrive_and_wait()
    // -----------------------------------------------------------------------

    /// Arrives at the barrier and waits until all `parties` participants
    /// have arrived.
    ///
    /// - `spin_max`: number of spins before timeout (0 = infinite)
    ///
    /// # Return
    /// - `Ok(BarrierResult::Leader)` — this thread was the last (releases others)
    /// - `Ok(BarrierResult::Follower)` — normal wakeup
    /// - `Err(IpcError::Timeout)` — timeout expired
    /// - `Err(IpcError::Closed)` — barrier destroyed during wait
    pub fn arrive_and_wait(&self, spin_max: u64) -> Result<BarrierResult, IpcError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }

        self.total_arrives.fetch_add(1, Ordering::Relaxed);

        // Save current generation BEFORE incrementing `arrived`
        let gen = self.generation.load(Ordering::Acquire);
        let parties = self.parties.load(Ordering::Relaxed);

        // Increment arrival counter
        let my_arrival = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;

        if my_arrival == parties {
            // This thread is LEADER: release all waiters
            self.cycles.fetch_add(1, Ordering::Relaxed);
            // Reset counter to zero
            self.arrived.store(0, Ordering::Relaxed);
            // Move to next generation — wakes all waiters
            self.generation.fetch_add(1, Ordering::Release);
            return Ok(BarrierResult::Leader);
        }

        // This thread must wait for generation to change
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }

            if !self.active.load(Ordering::Relaxed) {
                return Err(IpcError::Closed);
            }

            let current_gen = self.generation.load(Ordering::Acquire);
            if current_gen != gen {
                // Generation changed: barrier was crossed
                return Ok(BarrierResult::Follower);
            }

            if spins >= limit {
                // Check one last time if barrier was crossed before declaring timeout
                if self.generation.load(Ordering::Acquire) != gen {
                    return Ok(BarrierResult::Follower);
                }
                // T8-043 FIX: decrement arrived under CAS only if generation has not advanced
                let mut cur = self.arrived.load(Ordering::Acquire);
                while cur > 0 && self.generation.load(Ordering::Acquire) == gen {
                    match self.arrived.compare_exchange_weak(
                        cur,
                        cur - 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => break,
                        Err(actual) => cur = actual,
                    }
                }
                if self.generation.load(Ordering::Acquire) != gen {
                    return Ok(BarrierResult::Follower);
                }
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Non-blocking arrival
    // -----------------------------------------------------------------------

    /// Arrives at the barrier without waiting.
    /// Returns `true` if this thread completed the cycle (was the last).
    pub fn arrive(&self) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        let parties = self.parties.load(Ordering::Relaxed);
        let my_arrival = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;
        if my_arrival == parties {
            self.arrived.store(0, Ordering::Relaxed);
            self.cycles.fetch_add(1, Ordering::Relaxed);
            self.generation.fetch_add(1, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Waits for generation to advance (without incrementing `arrived`).
    ///
    /// Useful for non-blocking `arrive()` followed by a separate wait.
    pub fn wait_for_phase(&self, expected_gen: u32, spin_max: u64) -> Result<(), IpcError> {
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;
        loop {
            core::hint::spin_loop();
            spins += 1;
            if !self.active.load(Ordering::Relaxed) {
                return Err(IpcError::Closed);
            }
            if self.generation.load(Ordering::Acquire) != expected_gen {
                return Ok(());
            }
            if spins >= limit {
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Reset and destruction
    // -----------------------------------------------------------------------

    /// Resets the barrier (emergency force reset).
    /// Wakes all waiters via generation change.
    pub fn reset(&self) {
        self.arrived.store(0, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Marks barrier as destroyed. Current waiters will return Closed.
    pub fn destroy(&self) {
        self.active.store(false, Ordering::Release);
        // Release blocked waiters by advancing generation
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Retrieves statistics
    pub fn snapshot(&self) -> IpcBarrierStats {
        IpcBarrierStats {
            id: self.id,
            parties: self.parties.load(Ordering::Relaxed),
            arrived: self.arrived.load(Ordering::Relaxed),
            generation: self.generation.load(Ordering::Relaxed),
            cycles: self.cycles.load(Ordering::Relaxed),
            total_arrives: self.total_arrives.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcBarrierStats {
    pub id: u32,
    pub parties: u32,
    pub arrived: u32,
    pub generation: u32,
    pub cycles: u64,
    pub total_arrives: u64,
    pub total_timeouts: u64,
}

// ---------------------------------------------------------------------------
// Global IPC barrier table
// ---------------------------------------------------------------------------

struct BarrierSlot {
    barrier: MaybeUninit<IpcBarrier>,
    occupied: AtomicBool,
}

impl BarrierSlot {
    const fn empty() -> Self {
        Self {
            barrier: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct IpcBarrierTable {
    slots: [BarrierSlot; MAX_IPC_BARRIERS],
    count: AtomicU32,
}

// SAFETY: access via CAS + MaybeUninit
unsafe impl Sync for IpcBarrierTable {}

impl IpcBarrierTable {
    const fn new() -> Self {
        const EMPTY: BarrierSlot = BarrierSlot::empty();
        Self {
            slots: [EMPTY; MAX_IPC_BARRIERS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32, parties: u32) -> Option<usize> {
        if parties == 0 || parties as usize > MAX_BARRIER_PARTIES {
            return None;
        }
        for i in 0..MAX_IPC_BARRIERS {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i]
                    .occupied
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    let ptr = self.slots[i].barrier.as_ptr() as *mut IpcBarrier;
                    // SAFETY: CAS AcqRel guarantees slot exclusivity; MaybeUninit<IpcBarrier> write-once.
                    unsafe { ptr.write(IpcBarrier::new(id, parties)) }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&IpcBarrier> {
        if idx >= MAX_IPC_BARRIERS {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: occupied=true guarantees initialization
        Some(unsafe { &*self.slots[idx].barrier.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_IPC_BARRIERS {
            return false;
        }
        if let Some(b) = self.get(idx) {
            b.destroy();
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

static IPC_BARRIER_TABLE: IpcBarrierTable = IpcBarrierTable::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Creates a barrier for `parties` participants. Returns index (handle).
pub fn barrier_create(id: u32, parties: u32) -> Option<usize> {
    IPC_BARRIER_TABLE.alloc(id, parties)
}

/// Arrives and waits until all participants are present.
pub fn barrier_arrive_and_wait(idx: usize, spin_max: u64) -> Result<BarrierResult, IpcError> {
    IPC_BARRIER_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .arrive_and_wait(spin_max)
}

/// Arrives without waiting. Returns true if this thread completes the cycle.
pub fn barrier_arrive(idx: usize) -> Result<bool, IpcError> {
    Ok(IPC_BARRIER_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .arrive())
}

/// Waits for expected_gen generation to advance.
pub fn barrier_wait_phase(idx: usize, expected_gen: u32, spin_max: u64) -> Result<(), IpcError> {
    IPC_BARRIER_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .wait_for_phase(expected_gen, spin_max)
}

/// Returns current generation (for non-blocking arrival).
pub fn barrier_generation(idx: usize) -> Option<u32> {
    IPC_BARRIER_TABLE.get(idx).map(|b| b.current_generation())
}

/// Emergency resets the barrier.
pub fn barrier_reset(idx: usize) -> Result<(), IpcError> {
    IPC_BARRIER_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)
        .map(|b| b.reset())
}

/// Destroys barrier and frees its slot.
pub fn barrier_destroy(idx: usize) -> bool {
    IPC_BARRIER_TABLE.free(idx)
}

/// Number of active barriers.
pub fn barrier_count() -> u32 {
    IPC_BARRIER_TABLE.count()
}

/// Statistics snapshot.
pub fn barrier_stats(idx: usize) -> Option<IpcBarrierStats> {
    IPC_BARRIER_TABLE.get(idx).map(|b| b.snapshot())
}
