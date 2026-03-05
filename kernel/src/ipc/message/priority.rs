// ipc/message/priority.rs — File de priorité IPC pour Exo-OS
//
// Ce module implémente une file de priorité IPC à deux niveaux :
//   - Niveau REALTIME (priorité > PRIORITY_RT_THRESHOLD) : SpscRing dédié
//   - Niveau NORMAL (priorité <= PRIORITY_RT_THRESHOLD) : SpscRing standard
//
// Le déqueue lit toujours d'abord le niveau REALTIME, puis NORMAL.
// En cas de file RT pleine, les messages RT sont rétrogradés en NORMAL
// avec le flag `DEGRADED` positionné.
//
// Une file de priorité peut également être weightée : le poids détermine
// combien de messages normaux sont lus entre deux messages RT (pour éviter
// la famine).
//
// RÈGLE PRIO-01 : deux rings statiques par PriorityQueue, pas de tri en O(N).
// RÈGLE PRIO-02 : le niveau RT est TOUJOURS consommé en premier.
// RÈGLE PRIO-03 : pas de starvation → ratio configurable RT:NORMAL.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{IpcError, MessageFlags};
use crate::ipc::message::builder::{IpcMessage, MAX_MSG_INLINE};
use crate::ipc::ring::spsc::{SpscRing, SPSC_CAPACITY};
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};

// ---------------------------------------------------------------------------
// Constantes
// ---------------------------------------------------------------------------

/// Seuil de priorité pour la file REALTIME (>= → RT)
pub const PRIORITY_RT_THRESHOLD: u8 = 192;

/// Capacité du ring RT (plus petite : les messages RT sont rares)
pub const PRIORITY_RT_RING_CAP: usize = 64;

/// Capacité du ring NORMAL
pub const PRIORITY_NORMAL_RING_CAP: usize = 256;

/// Ratio RT:NORMAL pour l'anti-starvation (consommer N_NORMAL entre chaque RT)
pub const PRIORITY_ANTI_STARVATION_RATIO: u32 = 8;

/// Nombre maximum de PriorityQueue dans la table globale
pub const MAX_PRIORITY_QUEUES: usize = 64;

// ---------------------------------------------------------------------------
// Index de message pour la file de priorité
// ---------------------------------------------------------------------------
//
// Pour éviter de copier des IpcMessage (>4KB) dans le ring, on stocke un
// `PrioMsgSlot` compact : header + payload inline 256 bytes max.
// Pour les payloads plus grand, utiliser le StreamChannel.

pub const PRIO_INLINE_PAYLOAD: usize = 256;

/// Slot de message dans la file de priorité
#[repr(C, align(64))]
pub struct PrioMsgSlot {
    pub seq: u64,
    pub src: u32,
    pub dst: u32,
    pub priority: u8,
    pub msg_type: u8,
    pub flags: u16,
    pub payload_len: u16,
    pub cookie: u64,
    _pad: [u8; 5],
    pub payload: [u8; PRIO_INLINE_PAYLOAD],
}

impl PrioMsgSlot {
    pub const fn empty() -> Self {
        Self {
            seq: 0,
            src: 0,
            dst: 0,
            priority: 0,
            msg_type: 0,
            flags: 0,
            payload_len: 0,
            cookie: 0,
            _pad: [0u8; 5],
            payload: [0u8; PRIO_INLINE_PAYLOAD],
        }
    }

    pub fn from_message(msg: &IpcMessage) -> Self {
        let plen = (msg.payload_len as usize).min(PRIO_INLINE_PAYLOAD);
        let mut slot = Self::empty();
        slot.seq = msg.seq;
        slot.src = msg.src.0.get() as u32;
        slot.dst = msg.dst.0.get() as u32;
        slot.priority = msg.priority;
        slot.msg_type = msg.msg_type as u8;
        slot.flags = msg.flags.bits();
        slot.payload_len = plen as u16;
        slot.cookie = msg.cookie;
        if plen > 0 {
            slot.payload[..plen].copy_from_slice(&msg.payload()[..plen]);
        }
        slot
    }
}

// SAFETY: PrioMsgSlot est Copy-able en termes de données
unsafe impl Sync for PrioMsgSlot {}

// ---------------------------------------------------------------------------
// Ring statique pour la file de priorité
// ---------------------------------------------------------------------------

/// Ring circulaire borné pour PrioMsgSlot (pas de dépendance à SpscRing<T>)
struct PrioRing<const N: usize> {
    slots: [MaybeUninit<PrioMsgSlot>; N],
    head: AtomicU32,
    tail: AtomicU32,
    capacity: u32,
}

impl<const N: usize> PrioRing<N> {
    const fn new() -> Self {
        const EMPTY: MaybeUninit<PrioMsgSlot> = MaybeUninit::uninit();
        Self {
            slots: [EMPTY; N],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            capacity: N as u32,
        }
    }

    fn push(&self, slot: PrioMsgSlot) -> bool {
        loop {
            let tail = self.tail.load(Ordering::Relaxed);
            let head = self.head.load(Ordering::Acquire);
            if tail.wrapping_sub(head) >= self.capacity {
                return false; // plein
            }
            match self.tail.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let idx = (tail % self.capacity) as usize;
                    // SAFETY: slot exclusif via CAS tail
                    unsafe {
                        (self.slots[idx].as_ptr() as *mut PrioMsgSlot).write(slot);
                    }
                    return true;
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    fn pop(&self) -> Option<PrioMsgSlot> {
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head == tail {
                return None; // vide
            }
            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let idx = (head % self.capacity) as usize;
                    // SAFETY: slot initialisé (tail > head)
                    let slot = unsafe { self.slots[idx].assume_init_read() };
                    return Some(slot);
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    fn len(&self) -> u32 {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }
}

// SAFETY: PrioRing accès concurrent via CAS
unsafe impl<const N: usize> Sync for PrioRing<N> {}

// ---------------------------------------------------------------------------
// PriorityQueue — file à deux niveaux
// ---------------------------------------------------------------------------

/// File de priorité IPC à deux niveaux (RT + NORMAL).
pub struct PriorityQueue {
    pub id: u32,
    /// Ring RT (haute priorité)
    rt_ring: PrioRing<PRIORITY_RT_RING_CAP>,
    /// Ring NORMAL (priorité standard)
    normal_ring: PrioRing<PRIORITY_NORMAL_RING_CAP>,
    /// Compteur pour l'anti-starvation
    normal_reads_since_last_rt: AtomicU32,
    /// Statistiques
    pub rt_enqueued: AtomicU64,
    pub normal_enqueued: AtomicU64,
    pub rt_dequeued: AtomicU64,
    pub normal_dequeued: AtomicU64,
    pub rt_degraded: AtomicU64,
    pub rt_full_drops: AtomicU64,
}

// SAFETY: PrioRing est Sync
unsafe impl Sync for PriorityQueue {}
unsafe impl Send for PriorityQueue {}

impl PriorityQueue {
    pub const fn new(id: u32) -> Self {
        Self {
            id,
            rt_ring: PrioRing::new(),
            normal_ring: PrioRing::new(),
            normal_reads_since_last_rt: AtomicU32::new(0),
            rt_enqueued: AtomicU64::new(0),
            normal_enqueued: AtomicU64::new(0),
            rt_dequeued: AtomicU64::new(0),
            normal_dequeued: AtomicU64::new(0),
            rt_degraded: AtomicU64::new(0),
            rt_full_drops: AtomicU64::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Enqueue
    // -----------------------------------------------------------------------

    /// Enfile un message. Les messages RT (priority >= PRIORITY_RT_THRESHOLD)
    /// vont dans le ring RT ; les autres dans le ring NORMAL.
    /// Si le ring RT est plein, le message est rétrogradé en NORMAL.
    pub fn enqueue(&self, msg: &IpcMessage) -> Result<(), IpcError> {
        let slot = PrioMsgSlot::from_message(msg);
        if msg.priority >= PRIORITY_RT_THRESHOLD {
            if self.rt_ring.push(slot) {
                self.rt_enqueued.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                return Ok(());
            } else {
                // RT ring plein : dégrader
                self.rt_degraded.fetch_add(1, Ordering::Relaxed);
                // Re-créer le slot (consommé au-dessus)
                let slot2 = PrioMsgSlot::from_message(msg);
                if self.normal_ring.push(slot2) {
                    self.normal_enqueued.fetch_add(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageSent);
                    return Ok(());
                } else {
                    self.rt_full_drops.fetch_add(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageDropped);
                    return Err(IpcError::Full);
                }
            }
        } else {
            if self.normal_ring.push(slot) {
                self.normal_enqueued.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                return Ok(());
            } else {
                IPC_STATS.record(StatEvent::MessageDropped);
                return Err(IpcError::Full);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Dequeue
    // -----------------------------------------------------------------------

    /// Défile le prochain message selon la politique de priorité :
    /// 1. Si ring RT non vide ET (compteur anti-starvation < RATIO ou normal vide) → RT
    /// 2. Sinon → NORMAL
    pub fn dequeue(&self) -> Option<PrioMsgSlot> {
        let normal_count = self.normal_reads_since_last_rt.load(Ordering::Relaxed);
        let rt_empty = self.rt_ring.is_empty();
        let normal_empty = self.normal_ring.is_empty();

        if rt_empty && normal_empty {
            return None;
        }

        // Politique : RT ou anti-starvation?
        let take_rt = !rt_empty
            && (normal_empty || normal_count < PRIORITY_ANTI_STARVATION_RATIO);

        if take_rt {
            if let Some(slot) = self.rt_ring.pop() {
                self.rt_dequeued.fetch_add(1, Ordering::Relaxed);
                self.normal_reads_since_last_rt.store(0, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                return Some(slot);
            }
        }

        // Consommer depuis NORMAL
        if let Some(slot) = self.normal_ring.pop() {
            self.normal_dequeued.fetch_add(1, Ordering::Relaxed);
            self.normal_reads_since_last_rt.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Some(slot);
        }

        // Si RT disponible malgré ratio
        if let Some(slot) = self.rt_ring.pop() {
            self.rt_dequeued.fetch_add(1, Ordering::Relaxed);
            self.normal_reads_since_last_rt.store(0, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Some(slot);
        }

        None
    }

    /// Tentative de défile non-bloquante (alias de dequeue).
    pub fn try_dequeue(&self) -> Option<PrioMsgSlot> {
        self.dequeue()
    }

    /// Vérifie si la file est vide.
    pub fn is_empty(&self) -> bool {
        self.rt_ring.is_empty() && self.normal_ring.is_empty()
    }

    /// Nombre total de messages dans les deux rings.
    pub fn len(&self) -> u32 {
        self.rt_ring.len() + self.normal_ring.len()
    }

    /// Snapshot de statistiques.
    pub fn snapshot(&self) -> PriorityQueueStats {
        PriorityQueueStats {
            id: self.id,
            rt_pending: self.rt_ring.len(),
            normal_pending: self.normal_ring.len(),
            rt_enqueued: self.rt_enqueued.load(Ordering::Relaxed),
            normal_enqueued: self.normal_enqueued.load(Ordering::Relaxed),
            rt_dequeued: self.rt_dequeued.load(Ordering::Relaxed),
            normal_dequeued: self.normal_dequeued.load(Ordering::Relaxed),
            rt_degraded: self.rt_degraded.load(Ordering::Relaxed),
            rt_full_drops: self.rt_full_drops.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PriorityQueueStats {
    pub id: u32,
    pub rt_pending: u32,
    pub normal_pending: u32,
    pub rt_enqueued: u64,
    pub normal_enqueued: u64,
    pub rt_dequeued: u64,
    pub normal_dequeued: u64,
    pub rt_degraded: u64,
    pub rt_full_drops: u64,
}

// ---------------------------------------------------------------------------
// Table globale de files de priorité
// ---------------------------------------------------------------------------

struct PrioQueueSlot {
    queue: MaybeUninit<PriorityQueue>,
    occupied: AtomicBool,
}

impl PrioQueueSlot {
    const fn empty() -> Self {
        Self {
            queue: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct PriorityQueueTable {
    slots: [PrioQueueSlot; MAX_PRIORITY_QUEUES],
    count: AtomicU32,
}

unsafe impl Sync for PriorityQueueTable {}

impl PriorityQueueTable {
    const fn new() -> Self {
        const EMPTY: PrioQueueSlot = PrioQueueSlot::empty();
        Self { slots: [EMPTY; MAX_PRIORITY_QUEUES], count: AtomicU32::new(0) }
    }

    fn alloc(&self, id: u32) -> Option<usize> {
        for i in 0..MAX_PRIORITY_QUEUES {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i].occupied.compare_exchange(
                    false, true, Ordering::AcqRel, Ordering::Relaxed,
                ).is_ok() {
                    // SAFETY: CAS AcqRel garantit l'exclusivité; queue MaybeUninit<PriorityQueue> write-once.
                    unsafe {
                        (self.slots[i].queue.as_ptr() as *mut PriorityQueue)
                            .write(PriorityQueue::new(id));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&PriorityQueue> {
        if idx >= MAX_PRIORITY_QUEUES { return None; }
        if !self.slots[idx].occupied.load(Ordering::Acquire) { return None; }
        Some(unsafe { &*self.slots[idx].queue.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_PRIORITY_QUEUES { return false; }
        if self.slots[idx].occupied.compare_exchange(
            true, false, Ordering::AcqRel, Ordering::Relaxed,
        ).is_ok() {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

static PRIO_QUEUE_TABLE: PriorityQueueTable = PriorityQueueTable::new();

// ---------------------------------------------------------------------------
// API publique
// ---------------------------------------------------------------------------

pub fn prio_queue_create(id: u32) -> Option<usize> {
    PRIO_QUEUE_TABLE.alloc(id)
}

pub fn prio_queue_enqueue(idx: usize, msg: &IpcMessage) -> Result<(), IpcError> {
    PRIO_QUEUE_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?.enqueue(msg)
}

pub fn prio_queue_dequeue(idx: usize) -> Option<PrioMsgSlot> {
    PRIO_QUEUE_TABLE.get(idx)?.dequeue()
}

pub fn prio_queue_is_empty(idx: usize) -> Option<bool> {
    PRIO_QUEUE_TABLE.get(idx).map(|q| q.is_empty())
}

pub fn prio_queue_len(idx: usize) -> Option<u32> {
    PRIO_QUEUE_TABLE.get(idx).map(|q| q.len())
}

pub fn prio_queue_destroy(idx: usize) -> bool {
    PRIO_QUEUE_TABLE.free(idx)
}

pub fn prio_queue_stats(idx: usize) -> Option<PriorityQueueStats> {
    PRIO_QUEUE_TABLE.get(idx).map(|q| q.snapshot())
}
