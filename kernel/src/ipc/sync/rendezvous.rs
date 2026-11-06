// ipc/sync/rendezvous.rs — Generalized IPC rendezvous point for Exo-OS
//
// This module implements an N-way rendezvous:
// all N threads must arrive at the rendezvous point before any of them
// can proceed. Unlike a cyclic barrier, the rendezvous
// is single-use by default, but can be rearmed explicitly.
//
// Available variant: RendezvousExchange<T: Copy> allowing two threads
// to simultaneously exchange a value (bilateral symmetric rendezvous).
//
// RULE RNDV-01: no allocation. Static tables only.
// RULE RNDV-02: atomic exchange via two-phase CAS (offer/take).
// RULE RNDV-03: bounded spin-wait. Timeout -> clean deregistration.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// N-way rendezvous
// ---------------------------------------------------------------------------

/// Maximum number of participants per rendezvous
pub const MAX_RENDEZVOUS_PARTIES: usize = 64;

/// Maximum number of rendezvous in the global table
pub const MAX_RENDEZVOUS_ENTRIES: usize = 64;

/// Rendezvous state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RendezvousState {
    /// Waiting for all participants
    Waiting = 0,
    /// All participants have arrived — release in progress
    Releasing = 1,
    /// Destroyed / invalidated
    Destroyed = 2,
}

impl RendezvousState {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Releasing,
            2 => Self::Destroyed,
            _ => Self::Waiting,
        }
    }
}

/// Single-use N-way IPC rendezvous (can be rearmed).
#[repr(C, align(64))]
pub struct IpcRendezvous {
    pub id: u32,
    /// Number of required participants
    parties: AtomicU32,
    /// Number of arrived participants
    arrived: AtomicU32,
    /// Internal state
    state: AtomicU32,
    /// Generation (anti-spurious wakeup)
    generation: AtomicU32,
    /// Active (not destroyed)
    pub active: AtomicBool,
    _pad: [u8; 3],
    /// Statistics
    pub total_meetings: AtomicU64,
    pub total_timeouts: AtomicU64,
}

unsafe impl Sync for IpcRendezvous {}
unsafe impl Send for IpcRendezvous {}

impl IpcRendezvous {
    pub const fn new(id: u32, parties: u32) -> Self {
        Self {
            id,
            parties: AtomicU32::new(parties),
            arrived: AtomicU32::new(0),
            state: AtomicU32::new(RendezvousState::Waiting as u32),
            generation: AtomicU32::new(0),
            active: AtomicBool::new(true),
            _pad: [0u8; 3],
            total_meetings: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
        }
    }

    pub fn parties(&self) -> u32 {
        self.parties.load(Ordering::Relaxed)
    }

    pub fn arrived_count(&self) -> u32 {
        self.arrived.load(Ordering::Relaxed)
    }

    /// Arrives at the rendezvous and waits.
    ///
    /// Returns `Ok(true)` for the leader thread (last to arrive),
    /// `Ok(false)` for others.
    pub fn meet(&self, spin_max: u64) -> Result<bool, IpcError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }

        let gen = self.generation.load(Ordering::Acquire);
        let parties = self.parties.load(Ordering::Relaxed);
        let my_arrival = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;

        if my_arrival == parties {
            // This thread is the leader
            self.state
                .store(RendezvousState::Releasing as u32, Ordering::Release);
            self.total_meetings.fetch_add(1, Ordering::Relaxed);
            // Advance generation to release others
            self.generation.fetch_add(1, Ordering::Release);
            return Ok(true);
        }

        // Wait for leader to release
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

            if self.generation.load(Ordering::Acquire) != gen {
                return Ok(false);
            }

            if spins >= limit {
                if self.generation.load(Ordering::Acquire) != gen {
                    return Ok(false);
                }
                // Timeout: decrement without wrapping if generation has not advanced (T8-043)
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
                    return Ok(false);
                }
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Rearms the rendezvous for a next cycle.
    pub fn rearm(&self) {
        self.arrived.store(0, Ordering::Release);
        self.state
            .store(RendezvousState::Waiting as u32, Ordering::Release);
    }

    /// Destroys the rendezvous.
    pub fn destroy(&self) {
        self.active.store(false, Ordering::Release);
        self.state
            .store(RendezvousState::Destroyed as u32, Ordering::Release);
        // Release any waiters
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self) -> IpcRendezvousStats {
        IpcRendezvousStats {
            id: self.id,
            parties: self.parties.load(Ordering::Relaxed),
            arrived: self.arrived.load(Ordering::Relaxed),
            total_meetings: self.total_meetings.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcRendezvousStats {
    pub id: u32,
    pub parties: u32,
    pub arrived: u32,
    pub total_meetings: u64,
    pub total_timeouts: u64,
}

// ---------------------------------------------------------------------------
// 2-way symmetric exchange: RendezvousExchange<T>
// ---------------------------------------------------------------------------

/// Maximum size of an exchangeable value
pub const MAX_EXCHANGE_SIZE: usize = 512;

/// Bilateral exchange state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExchangeState {
    /// No one is waiting
    Empty = 0,
    /// Thread A is preparing its offer in offer_buf (T8-016 FIX)
    WritingOffer = 1,
    /// Thread A deposited its value and is waiting
    OfferPending = 2,
    /// Thread B retrieved, deposit in progress
    Completing = 3,
}

/// Inline exchange slot (no pointer, copied data)
#[repr(C, align(64))]
pub struct ExchangeSlot {
    state: AtomicU32,
    /// Offering thread identity (for exclusion)
    offerer_thread: AtomicU32,
    /// Data size (for validation)
    data_size: AtomicU32,
    /// Offer buffer (Thread A data)
    offer_buf: [u8; MAX_EXCHANGE_SIZE],
    /// Reply buffer (Thread B data)
    reply_buf: [u8; MAX_EXCHANGE_SIZE],
    /// Result available (Thread A waiting for reply_buf)
    result_ready: AtomicBool,
    _pad: [u8; 3],
    /// Statistics
    pub exchanges: AtomicU64,
    pub timeouts: AtomicU64,
}

unsafe impl Sync for ExchangeSlot {}
unsafe impl Send for ExchangeSlot {}

impl ExchangeSlot {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(ExchangeState::Empty as u32),
            offerer_thread: AtomicU32::new(0),
            data_size: AtomicU32::new(0),
            offer_buf: [0u8; MAX_EXCHANGE_SIZE],
            reply_buf: [0u8; MAX_EXCHANGE_SIZE],
            result_ready: AtomicBool::new(false),
            _pad: [0u8; 3],
            exchanges: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
        }
    }

    /// Data exchange between two threads.
    ///
    /// Thread A calls `exchange(my_data, out_buf)` -> blocks until
    /// Thread B also calls `exchange(...)`.
    ///
    /// Both threads return with the other's data.
    ///
    /// `data` must have the same `size` for both threads.
    pub fn exchange(
        &self,
        thread_id: u32,
        data: &[u8],
        out: &mut [u8],
        spin_max: u64,
    ) -> Result<(), IpcError> {
        if data.len() > MAX_EXCHANGE_SIZE || out.len() < data.len() {
            return Err(IpcError::Invalid);
        }

        let size = data.len() as u32;

        // Attempt to become offerer (CAS Empty -> WritingOffer)
        let became_offerer = self
            .state
            .compare_exchange(
                ExchangeState::Empty as u32,
                ExchangeState::WritingOffer as u32,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok();

        if became_offerer {
            // --- Thread A: deposit offer ---
            self.offerer_thread.store(thread_id, Ordering::Relaxed);
            self.data_size.store(size, Ordering::Relaxed);

            // Copy my data into offer_buf
            // SAFETY: offer_buf is an inline array, data.len() <= MAX_EXCHANGE_SIZE
            unsafe {
                let dst = self.offer_buf.as_ptr() as *mut u8;
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            }

            self.result_ready.store(false, Ordering::Release);
            // T8-016 FIX: publish OfferPending only after data is fully written
            self.state
                .store(ExchangeState::OfferPending as u32, Ordering::Release);

            // Wait for Thread B to complete exchange
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
                if self.result_ready.load(Ordering::Acquire) {
                    // Read B's response in reply_buf
                    // SAFETY: B wrote reply_buf before setting result_ready
                    let recv_len = data.len().min(out.len());
                    unsafe {
                        let src = self.reply_buf.as_ptr();
                        core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), recv_len);
                    }
                    self.exchanges.fetch_add(1, Ordering::Relaxed);
                    // Reset result_ready and state after Thread A finished reading reply_buf
                    self.result_ready.store(false, Ordering::Release);
                    self.state
                        .store(ExchangeState::Empty as u32, Ordering::Release);
                    return Ok(());
                }
                if spins >= limit {
                    // T8-016 FIX: cancel offer only if Thread B is not already completing
                    if self
                        .state
                        .compare_exchange(
                            ExchangeState::OfferPending as u32,
                            ExchangeState::Empty as u32,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        self.timeouts.fetch_add(1, Ordering::Relaxed);
                        return Err(IpcError::Timeout);
                    }
                }
            }
        } else {
            // --- Thread B: attempt to take offer ---
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

                let st = self.state.load(Ordering::Acquire);
                if st == ExchangeState::OfferPending as u32 {
                    // CAS OfferPending -> Completing
                    if self
                        .state
                        .compare_exchange(
                            ExchangeState::OfferPending as u32,
                            ExchangeState::Completing as u32,
                            Ordering::AcqRel,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        // Read A's offer
                        // SAFETY: A initialized offer_buf under OfferPending
                        let offer_size = self.data_size.load(Ordering::Relaxed) as usize;
                        let recv_len = offer_size.min(out.len());
                        // SAFETY: offer_buf initialized by A under OfferPending (Acquire/Release on state).
                        unsafe {
                            let src = self.offer_buf.as_ptr();
                            core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), recv_len);
                        }

                        // Write our reply in reply_buf
                        let write_len = data.len().min(MAX_EXCHANGE_SIZE);
                        // SAFETY: reply_buf exclusive in Completing state — only B writes here.
                        unsafe {
                            let dst = self.reply_buf.as_ptr() as *mut u8;
                            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, write_len);
                        }

                        // Signal to A that exchange is ready (A will reset state to Empty after reading)
                        self.result_ready.store(true, Ordering::Release);
                        return Ok(());
                    }
                }

                if spins >= limit {
                    self.timeouts.fetch_add(1, Ordering::Relaxed);
                    return Err(IpcError::Timeout);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global tables
// ---------------------------------------------------------------------------

struct RendezvousTableEntry {
    rdv: MaybeUninit<IpcRendezvous>,
    occupied: AtomicBool,
}

impl RendezvousTableEntry {
    const fn empty() -> Self {
        Self {
            rdv: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct IpcRendezvousTable {
    slots: [RendezvousTableEntry; MAX_RENDEZVOUS_ENTRIES],
    count: AtomicU32,
}

unsafe impl Sync for IpcRendezvousTable {}

impl IpcRendezvousTable {
    const fn new() -> Self {
        const EMPTY: RendezvousTableEntry = RendezvousTableEntry::empty();
        Self {
            slots: [EMPTY; MAX_RENDEZVOUS_ENTRIES],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32, parties: u32) -> Option<usize> {
        if parties == 0 || parties as usize > MAX_RENDEZVOUS_PARTIES {
            return None;
        }
        for i in 0..MAX_RENDEZVOUS_ENTRIES {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i]
                    .occupied
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel guarantees slot exclusivity; rdv MaybeUninit write-once.
                    unsafe {
                        (self.slots[i].rdv.as_ptr() as *mut IpcRendezvous)
                            .write(IpcRendezvous::new(id, parties));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&IpcRendezvous> {
        if idx >= MAX_RENDEZVOUS_ENTRIES {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        Some(unsafe { &*self.slots[idx].rdv.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_RENDEZVOUS_ENTRIES {
            return false;
        }
        if let Some(r) = self.get(idx) {
            r.destroy();
        }
        if self.slots[idx]
            .occupied
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

/// Maximum number of symmetric exchanges in the table
pub const MAX_EXCHANGE_SLOTS: usize = 64;

struct ExchangeTable {
    slots: [MaybeUninit<ExchangeSlot>; MAX_EXCHANGE_SLOTS],
    occupied: [AtomicBool; MAX_EXCHANGE_SLOTS],
    count: AtomicU32,
}

unsafe impl Sync for ExchangeTable {}

impl ExchangeTable {
    const fn new() -> Self {
        const UNINIT: MaybeUninit<ExchangeSlot> = MaybeUninit::uninit();
        const FALSE: AtomicBool = AtomicBool::new(false);
        Self {
            slots: [UNINIT; MAX_EXCHANGE_SLOTS],
            occupied: [FALSE; MAX_EXCHANGE_SLOTS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self) -> Option<usize> {
        for i in 0..MAX_EXCHANGE_SLOTS {
            if !self.occupied[i].load(Ordering::Relaxed) {
                if self.occupied[i]
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel guarantees exclusivity; slots[i] MaybeUninit<ExchangeSlot> write-once.
                    unsafe {
                        (self.slots[i].as_ptr() as *mut ExchangeSlot).write(ExchangeSlot::new());
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&ExchangeSlot> {
        if idx >= MAX_EXCHANGE_SLOTS {
            return None;
        }
        if !self.occupied[idx].load(Ordering::Acquire) {
            return None;
        }
        Some(unsafe { &*self.slots[idx].as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_EXCHANGE_SLOTS {
            return false;
        }
        if self.occupied[idx]
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

static IPC_RENDEZVOUS_TABLE: IpcRendezvousTable = IpcRendezvousTable::new();
static IPC_EXCHANGE_TABLE: ExchangeTable = ExchangeTable::new();

// ---------------------------------------------------------------------------
// Public API: N-way rendezvous
// ---------------------------------------------------------------------------

/// Creates a rendezvous for `parties` participants. Returns index (handle).
pub fn rendezvous_create(id: u32, parties: u32) -> Option<usize> {
    IPC_RENDEZVOUS_TABLE.alloc(id, parties)
}

/// Waits until all `parties` are present.
/// Returns `true` if this thread was the last (leader).
pub fn rendezvous_meet(idx: usize, spin_max: u64) -> Result<bool, IpcError> {
    IPC_RENDEZVOUS_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .meet(spin_max)
}

/// Rearms the rendezvous for a next cycle.
pub fn rendezvous_rearm(idx: usize) -> Result<(), IpcError> {
    IPC_RENDEZVOUS_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)
        .map(|r| r.rearm())
}

/// Destroys rendezvous and frees its slot.
pub fn rendezvous_destroy(idx: usize) -> bool {
    IPC_RENDEZVOUS_TABLE.free(idx)
}

/// Retrieves number of arrived participants.
pub fn rendezvous_arrived(idx: usize) -> Option<u32> {
    IPC_RENDEZVOUS_TABLE.get(idx).map(|r| r.arrived_count())
}

/// Rendezvous statistics.
pub fn rendezvous_stats(idx: usize) -> Option<IpcRendezvousStats> {
    IPC_RENDEZVOUS_TABLE.get(idx).map(|r| r.snapshot())
}

// ---------------------------------------------------------------------------
// Public API: symmetric exchange
// ---------------------------------------------------------------------------

/// Creates a symmetric exchange slot.
pub fn exchange_create() -> Option<usize> {
    IPC_EXCHANGE_TABLE.alloc()
}

/// Exchanges data between two threads.
///
/// Both threads call this function simultaneously.
/// Each receives the other's data in `out`.
pub fn exchange_swap(
    idx: usize,
    thread_id: u32,
    data: &[u8],
    out: &mut [u8],
    spin_max: u64,
) -> Result<(), IpcError> {
    IPC_EXCHANGE_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .exchange(thread_id, data, out, spin_max)
}

/// Destroys an exchange slot.
pub fn exchange_destroy(idx: usize) -> bool {
    IPC_EXCHANGE_TABLE.free(idx)
}
