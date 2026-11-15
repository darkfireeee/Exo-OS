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

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

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
    /// Packed `(generation: u32) << 32 | (arrived: u32)` preventing reset/phase races
    gen_arrived: AtomicU64,
    /// Internal state
    state: AtomicU32,
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
    #[inline]
    const fn pack_ga(gen: u32, arrived: u32) -> u64 {
        ((gen as u64) << 32) | (arrived as u64)
    }

    pub const fn new(id: u32, parties: u32) -> Self {
        Self {
            id,
            parties: AtomicU32::new(parties),
            gen_arrived: AtomicU64::new(0),
            state: AtomicU32::new(RendezvousState::Waiting as u32),
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
        (self.gen_arrived.load(Ordering::Acquire) & 0xFFFF_FFFF) as u32
    }

    #[inline]
    fn current_generation(&self) -> u32 {
        (self.gen_arrived.load(Ordering::Acquire) >> 32) as u32
    }

    fn advance_generation_and_zero(&self) {
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

    /// Arrives at the rendezvous and waits.
    ///
    /// Returns `Ok(true)` for the leader thread (last to arrive),
    /// `Ok(false)` for others.
    pub fn meet(&self, spin_max: u64) -> Result<bool, IpcError> {
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;
        let parties = self.parties.load(Ordering::Relaxed);
        let gen: u32;

        loop {
            if !self.active.load(Ordering::Acquire) {
                return Err(IpcError::Closed);
            }
            if self.state.load(Ordering::Acquire) == RendezvousState::Waiting as u32 {
                let cur = self.gen_arrived.load(Ordering::Acquire);
                let cur_gen = (cur >> 32) as u32;
                let arrived = (cur & 0xFFFF_FFFF) as u32;
                if arrived < parties {
                    let next_arrived = arrived + 1;
                    if next_arrived >= parties {
                        let next = Self::pack_ga(cur_gen.wrapping_add(1), parties);
                        if self
                            .gen_arrived
                            .compare_exchange_weak(
                                cur,
                                next,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            self.state
                                .store(RendezvousState::Releasing as u32, Ordering::Release);
                            self.total_meetings.fetch_add(1, Ordering::Relaxed);
                            return Ok(true);
                        }
                    } else {
                        let next = Self::pack_ga(cur_gen, next_arrived);
                        if self
                            .gen_arrived
                            .compare_exchange_weak(
                                cur,
                                next,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            gen = cur_gen;
                            break;
                        }
                    }
                }
            }
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            if spins >= limit {
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }

        // Wait for leader to release (generation advances)
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
                return Ok(false);
            }

            if spins >= limit {
                loop {
                    let cur = self.gen_arrived.load(Ordering::Acquire);
                    let cur_gen = (cur >> 32) as u32;
                    if cur_gen != gen {
                        return Ok(false);
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
                    return Ok(false);
                }
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Rearms the rendezvous for a next cycle.
    pub fn rearm(&self) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        loop {
            let cur = self.gen_arrived.load(Ordering::Acquire);
            let cur_gen = (cur >> 32) as u32;
            let next = Self::pack_ga(cur_gen, 0);
            if self
                .gen_arrived
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
        self.state
            .store(RendezvousState::Waiting as u32, Ordering::Release);
    }

    /// Destroys the rendezvous.
    pub fn destroy(&self) {
        self.active.store(false, Ordering::Release);
        self.state
            .store(RendezvousState::Destroyed as u32, Ordering::Release);
        // Release any waiters
        self.advance_generation_and_zero();
    }

    pub fn snapshot(&self) -> IpcRendezvousStats {
        IpcRendezvousStats {
            id: self.id,
            parties: self.parties.load(Ordering::Relaxed),
            arrived: self.arrived_count(),
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
const EXCHANGE_ABORTED: u32 = u32::MAX;

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
    /// Reply size written by Thread B (or EXCHANGE_ABORTED on size mismatch)
    reply_size: AtomicU32,
    /// Offer buffer (Thread A data)
    offer_buf: UnsafeCell<[u8; MAX_EXCHANGE_SIZE]>,
    /// Reply buffer (Thread B data)
    reply_buf: UnsafeCell<[u8; MAX_EXCHANGE_SIZE]>,
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
            reply_size: AtomicU32::new(0),
            offer_buf: UnsafeCell::new([0u8; MAX_EXCHANGE_SIZE]),
            reply_buf: UnsafeCell::new([0u8; MAX_EXCHANGE_SIZE]),
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
            self.data_size.store(size, Ordering::Release);
            self.reply_size.store(0, Ordering::Relaxed);

            // SAFETY: exclusive state WritingOffer owns offer_buf; data.len() <= MAX_EXCHANGE_SIZE.
            unsafe {
                let dst = (*self.offer_buf.get()).as_mut_ptr();
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            }

            self.result_ready.store(false, Ordering::Release);
            // Publish OfferPending only after data is fully written
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
                    let rep_sz = self.reply_size.load(Ordering::Acquire);
                    if rep_sz == EXCHANGE_ABORTED || (rep_sz as usize) > out.len() {
                        self.result_ready.store(false, Ordering::Release);
                        self.state
                            .store(ExchangeState::Empty as u32, Ordering::Release);
                        return Err(IpcError::Invalid);
                    }
                    let recv_len = (rep_sz as usize).min(out.len());
                    // SAFETY: B wrote reply_buf and reply_size before setting result_ready
                    unsafe {
                        let src = (*self.reply_buf.get()).as_ptr();
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
                    // Cancel offer only if Thread B is not already completing
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
                        let offer_size = self.data_size.load(Ordering::Acquire) as usize;
                        if offer_size > MAX_EXCHANGE_SIZE
                            || offer_size > out.len()
                            || data.len() != offer_size
                        {
                            self.reply_size.store(EXCHANGE_ABORTED, Ordering::Release);
                            self.result_ready.store(true, Ordering::Release);
                            return Err(IpcError::Invalid);
                        }

                        // SAFETY: offer_buf initialized by A under OfferPending.
                        unsafe {
                            let src = (*self.offer_buf.get()).as_ptr();
                            core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), offer_size);
                        }

                        // SAFETY: reply_buf exclusive in Completing state — only B writes here.
                        unsafe {
                            let dst = (*self.reply_buf.get()).as_mut_ptr();
                            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
                        }

                        self.reply_size
                            .store(data.len() as u32, Ordering::Release);
                        // Signal to A that exchange is ready (A resets state to Empty)
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

const SLOT_FREE: u8 = 0;
const SLOT_INIT: u8 = 1;
const SLOT_ACTIVE: u8 = 2;
const SLOT_DESTROYING: u8 = 3;

struct RendezvousTableEntry {
    rdv: UnsafeCell<MaybeUninit<IpcRendezvous>>,
    state: AtomicU8,
    active_ops: AtomicU32,
}

impl RendezvousTableEntry {
    const fn empty() -> Self {
        Self {
            rdv: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(SLOT_FREE),
            active_ops: AtomicU32::new(0),
        }
    }
}

struct RendezvousRef<'a> {
    slot: &'a RendezvousTableEntry,
    inner: &'a IpcRendezvous,
}

impl<'a> Drop for RendezvousRef<'a> {
    fn drop(&mut self) {
        self.slot.active_ops.fetch_sub(1, Ordering::Release);
    }
}

impl<'a> core::ops::Deref for RendezvousRef<'a> {
    type Target = IpcRendezvous;
    fn deref(&self) -> &Self::Target {
        self.inner
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
            if self.slots[i].state.load(Ordering::Relaxed) == SLOT_FREE {
                if self.slots[i]
                    .state
                    .compare_exchange(SLOT_FREE, SLOT_INIT, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel SLOT_FREE->SLOT_INIT guarantees exclusivity before publish.
                    unsafe {
                        (*self.slots[i].rdv.get()).write(IpcRendezvous::new(id, parties));
                    }
                    self.slots[i].state.store(SLOT_ACTIVE, Ordering::Release);
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<RendezvousRef<'_>> {
        if idx >= MAX_RENDEZVOUS_ENTRIES {
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
        let inner = unsafe { (&*slot.rdv.get()).assume_init_ref() };
        Some(RendezvousRef { slot, inner })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_RENDEZVOUS_ENTRIES {
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
        unsafe { (&*slot.rdv.get()).assume_init_ref().destroy() };
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
}

/// Maximum number of symmetric exchanges in the table
pub const MAX_EXCHANGE_SLOTS: usize = 64;

struct ExchangeTableEntry {
    slot: UnsafeCell<MaybeUninit<ExchangeSlot>>,
    state: AtomicU8,
    active_ops: AtomicU32,
}

impl ExchangeTableEntry {
    const fn empty() -> Self {
        Self {
            slot: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(SLOT_FREE),
            active_ops: AtomicU32::new(0),
        }
    }
}

struct ExchangeRef<'a> {
    entry: &'a ExchangeTableEntry,
    inner: &'a ExchangeSlot,
}

impl<'a> Drop for ExchangeRef<'a> {
    fn drop(&mut self) {
        self.entry.active_ops.fetch_sub(1, Ordering::Release);
    }
}

impl<'a> core::ops::Deref for ExchangeRef<'a> {
    type Target = ExchangeSlot;
    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

struct ExchangeTable {
    entries: [ExchangeTableEntry; MAX_EXCHANGE_SLOTS],
    count: AtomicU32,
}

unsafe impl Sync for ExchangeTable {}

impl ExchangeTable {
    const fn new() -> Self {
        const EMPTY: ExchangeTableEntry = ExchangeTableEntry::empty();
        Self {
            entries: [EMPTY; MAX_EXCHANGE_SLOTS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self) -> Option<usize> {
        for i in 0..MAX_EXCHANGE_SLOTS {
            if self.entries[i].state.load(Ordering::Relaxed) == SLOT_FREE {
                if self.entries[i]
                    .state
                    .compare_exchange(SLOT_FREE, SLOT_INIT, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel SLOT_FREE->SLOT_INIT guarantees exclusivity before publish.
                    unsafe {
                        (*self.entries[i].slot.get()).write(ExchangeSlot::new());
                    }
                    self.entries[i].state.store(SLOT_ACTIVE, Ordering::Release);
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<ExchangeRef<'_>> {
        if idx >= MAX_EXCHANGE_SLOTS {
            return None;
        }
        let entry = &self.entries[idx];
        if entry.state.load(Ordering::Acquire) != SLOT_ACTIVE {
            return None;
        }
        entry.active_ops.fetch_add(1, Ordering::AcqRel);
        if entry.state.load(Ordering::Acquire) != SLOT_ACTIVE {
            entry.active_ops.fetch_sub(1, Ordering::Release);
            return None;
        }
        let inner = unsafe { (&*entry.slot.get()).assume_init_ref() };
        Some(ExchangeRef { entry, inner })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_EXCHANGE_SLOTS {
            return false;
        }
        let entry = &self.entries[idx];
        if entry
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
        let mut spins = 0u64;
        while entry.active_ops.load(Ordering::Acquire) > 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins % 128 == 0 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
        }
        entry.state.store(SLOT_FREE, Ordering::Release);
        self.count.fetch_sub(1, Ordering::Relaxed);
        true
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
