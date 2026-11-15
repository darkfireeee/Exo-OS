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

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

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
/// Reusable after each cycle thanks to packed `(generation, arrived)` counter.
#[repr(C, align(64))]
pub struct IpcBarrier {
    /// Opaque identifier
    pub id: u32,
    /// Number of participants required to cross the barrier
    parties: AtomicU32,
    /// Packed `(generation: u32) << 32 | (arrived: u32)` preventing cycle/reset races
    gen_arrived: AtomicU64,
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
    #[inline]
    const fn pack_ga(gen: u32, arrived: u32) -> u64 {
        ((gen as u64) << 32) | (arrived as u64)
    }

    pub const fn new(id: u32, parties: u32) -> Self {
        Self {
            id,
            parties: AtomicU32::new(parties),
            gen_arrived: AtomicU64::new(0),
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
        (self.gen_arrived.load(Ordering::Acquire) & 0xFFFF_FFFF) as u32
    }

    /// Returns current generation (phase)
    pub fn current_generation(&self) -> u32 {
        (self.gen_arrived.load(Ordering::Acquire) >> 32) as u32
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

        let parties = self.parties.load(Ordering::Relaxed);
        let gen: u32;
        loop {
            if !self.active.load(Ordering::Acquire) {
                return Err(IpcError::Closed);
            }
            let cur = self.gen_arrived.load(Ordering::Acquire);
            let cur_gen = (cur >> 32) as u32;
            let cur_arrived = (cur & 0xFFFF_FFFF) as u32;
            let next_arrived = cur_arrived + 1;
            if next_arrived >= parties {
                let next = Self::pack_ga(cur_gen.wrapping_add(1), 0);
                if self
                    .gen_arrived
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.cycles.fetch_add(1, Ordering::Relaxed);
                    return Ok(BarrierResult::Leader);
                }
            } else {
                let next = Self::pack_ga(cur_gen, next_arrived);
                if self
                    .gen_arrived
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    gen = cur_gen;
                    break;
                }
            }
            core::hint::spin_loop();
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

            if !self.active.load(Ordering::Acquire) {
                return Err(IpcError::Closed);
            }

            if self.current_generation() != gen {
                // Generation changed: barrier was crossed
                return Ok(BarrierResult::Follower);
            }

            if spins >= limit {
                // Atomically decrement `arrived` ONLY if still in `gen`
                loop {
                    let cur = self.gen_arrived.load(Ordering::Acquire);
                    let cur_gen = (cur >> 32) as u32;
                    if cur_gen != gen {
                        return Ok(BarrierResult::Follower);
                    }
                    let cur_arrived = (cur & 0xFFFF_FFFF) as u32;
                    if cur_arrived == 0 {
                        break;
                    }
                    let next = Self::pack_ga(gen, cur_arrived - 1);
                    if self
                        .gen_arrived
                        .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        break;
                    }
                    core::hint::spin_loop();
                }
                if self.current_generation() != gen {
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
        let parties = self.parties.load(Ordering::Relaxed);
        loop {
            if !self.active.load(Ordering::Acquire) {
                return false;
            }
            let cur = self.gen_arrived.load(Ordering::Acquire);
            let cur_gen = (cur >> 32) as u32;
            let cur_arrived = (cur & 0xFFFF_FFFF) as u32;
            let next_arrived = cur_arrived + 1;
            if next_arrived >= parties {
                let next = Self::pack_ga(cur_gen.wrapping_add(1), 0);
                if self
                    .gen_arrived
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.cycles.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            } else {
                let next = Self::pack_ga(cur_gen, next_arrived);
                if self
                    .gen_arrived
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return false;
                }
            }
            core::hint::spin_loop();
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
            if spins % 128 == 0 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            if !self.active.load(Ordering::Acquire) {
                return Err(IpcError::Closed);
            }
            if self.current_generation() != expected_gen {
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

    fn advance_generation(&self) {
        loop {
            let cur = self.gen_arrived.load(Ordering::Acquire);
            let gen = (cur >> 32) as u32;
            let next = Self::pack_ga(gen.wrapping_add(1), 0);
            if self
                .gen_arrived
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
    }

    /// Resets the barrier (emergency force reset).
    /// Wakes all waiters via generation change.
    pub fn reset(&self) {
        self.advance_generation();
    }

    /// Marks barrier as destroyed. Current waiters will return Closed.
    pub fn destroy(&self) {
        self.active.store(false, Ordering::Release);
        // Release blocked waiters by advancing generation
        self.advance_generation();
    }

    /// Retrieves statistics
    pub fn snapshot(&self) -> IpcBarrierStats {
        let cur = self.gen_arrived.load(Ordering::Acquire);
        IpcBarrierStats {
            id: self.id,
            parties: self.parties.load(Ordering::Relaxed),
            arrived: (cur & 0xFFFF_FFFF) as u32,
            generation: (cur >> 32) as u32,
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

const SLOT_FREE: u8 = 0;
const SLOT_INIT: u8 = 1;
const SLOT_ACTIVE: u8 = 2;
const SLOT_DESTROYING: u8 = 3;

struct BarrierSlot {
    barrier: UnsafeCell<MaybeUninit<IpcBarrier>>,
    state: AtomicU8,
    active_ops: AtomicU32,
}

impl BarrierSlot {
    const fn empty() -> Self {
        Self {
            barrier: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(SLOT_FREE),
            active_ops: AtomicU32::new(0),
        }
    }
}

struct BarrierRef<'a> {
    slot: &'a BarrierSlot,
    inner: &'a IpcBarrier,
}

impl<'a> Drop for BarrierRef<'a> {
    fn drop(&mut self) {
        self.slot.active_ops.fetch_sub(1, Ordering::Release);
    }
}

impl<'a> core::ops::Deref for BarrierRef<'a> {
    type Target = IpcBarrier;
    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

struct IpcBarrierTable {
    slots: [BarrierSlot; MAX_IPC_BARRIERS],
    count: AtomicU32,
}

// SAFETY: access via state machine CAS + UnsafeCell<MaybeUninit> + active_ops
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
            if self.slots[i].state.load(Ordering::Relaxed) == SLOT_FREE {
                if self.slots[i]
                    .state
                    .compare_exchange(SLOT_FREE, SLOT_INIT, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel SLOT_FREE->SLOT_INIT guarantees exclusivity before publish.
                    unsafe {
                        (*self.slots[i].barrier.get()).write(IpcBarrier::new(id, parties));
                    }
                    self.slots[i].state.store(SLOT_ACTIVE, Ordering::Release);
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<BarrierRef<'_>> {
        if idx >= MAX_IPC_BARRIERS {
            return None;
        }
        let slot = &self.slots[idx];
        if slot.state.load(Ordering::Acquire) != SLOT_ACTIVE {
            return None;
        }
        slot.active_ops.fetch_add(1, Ordering::AcqRel);
        if slot.state.load(Ordering::Acquire) != SLOT_ACTIVE {
            slot.active_ops.fetch_sub(1, Ordering::Release);
            return None;
        }
        // SAFETY: slot state is SLOT_ACTIVE and active_ops > 0 pins it against reuse.
        let inner = unsafe { (&*slot.barrier.get()).assume_init_ref() };
        Some(BarrierRef { slot, inner })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_IPC_BARRIERS {
            return false;
        }
        let slot = &self.slots[idx];
        if slot
            .state
            .compare_exchange(
                SLOT_ACTIVE,
                SLOT_DESTROYING,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return false;
        }
        // SAFETY: SLOT_ACTIVE->SLOT_DESTROYING keeps slot initialized until active_ops drains.
        unsafe { (&*slot.barrier.get()).assume_init_ref().destroy() };
        let mut spins = 0u64;
        while slot.active_ops.load(Ordering::Acquire) > 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
        }
        slot.state.store(SLOT_FREE, Ordering::Release);
        self.count.fetch_sub(1, Ordering::Relaxed);
        true
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
