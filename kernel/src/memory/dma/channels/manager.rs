// kernel/src/memory/dma/channels/manager.rs
//
// Gestionnaire de canaux DMA — tient la table des canaux enregistrés,
// orchestre l'allocation de canaux pour les périphériques demandeurs.
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::dma::core::types::{
    DmaChannelId, DmaCapabilities, DmaPriority, DmaError, DmaDirection, DmaMapFlags,
};
use crate::memory::dma::core::descriptor::{DmaDescriptor, DMA_DESCRIPTOR_TABLE};
use crate::memory::dma::core::types::DmaTransactionId;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_DMA_CHANNELS: usize = 128;
pub const CHANNEL_QUEUE_DEPTH: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// ÉTAT D'UN CANAL
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum ChannelState {
    Free       = 0,
    Idle       = 1,
    Running    = 2,
    Paused     = 3,
    Error      = 4,
    Terminating= 5,
}

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTEUR DE CANAL
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques d'un canal DMA.
pub struct ChannelStats {
    pub submitted:     AtomicU64,
    pub completed:     AtomicU64,
    pub errors:        AtomicU64,
    pub bytes_total:   AtomicU64,
    pub avg_latency_ns: AtomicU64,
}

impl ChannelStats {
    const fn new() -> Self {
        ChannelStats {
            submitted:      AtomicU64::new(0),
            completed:      AtomicU64::new(0),
            errors:         AtomicU64::new(0),
            bytes_total:    AtomicU64::new(0),
            avg_latency_ns: AtomicU64::new(0),
        }
    }
}

/// File d'attente des transactions d'un canal (ring buffer).
struct ChannelQueue {
    txns:  [DmaTransactionId; CHANNEL_QUEUE_DEPTH],
    head:  usize,
    tail:  usize,
    count: usize,
}

impl ChannelQueue {
    const fn new() -> Self {
        ChannelQueue {
            txns:  [DmaTransactionId::INVALID; CHANNEL_QUEUE_DEPTH],
            head:  0,
            tail:  0,
            count: 0,
        }
    }

    fn push(&mut self, txn: DmaTransactionId) -> bool {
        if self.count >= CHANNEL_QUEUE_DEPTH { return false; }
        self.txns[self.tail] = txn;
        self.tail = (self.tail + 1) % CHANNEL_QUEUE_DEPTH;
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<DmaTransactionId> {
        if self.count == 0 { return None; }
        let txn = self.txns[self.head];
        self.head = (self.head + 1) % CHANNEL_QUEUE_DEPTH;
        self.count -= 1;
        Some(txn)
    }

    fn is_empty(&self) -> bool { self.count == 0 }
    fn is_full(&self)  -> bool { self.count >= CHANNEL_QUEUE_DEPTH }
    fn len(&self)      -> usize { self.count }
}

/// Un canal DMA.
#[repr(C, align(64))]
pub struct DmaChannel {
    /// Identifiant du canal.
    pub id:           DmaChannelId,
    /// Capacités déclarées par ce canal.
    pub capabilities: DmaCapabilities,
    /// Priorité du canal.
    pub priority:     DmaPriority,
    /// État courant.
    state:            AtomicU32,   // ChannelState as u32
    /// Ce canal est actif (enregistré) ?
    pub registered:   AtomicBool,
    /// Affinité CPU recommandée (u8::MAX = aucune préférence).
    pub cpu_affinity: u8,
    _pad: [u8; 2],
    /// File d'attente des transactions.
    queue:  Mutex<ChannelQueue>,
    /// Statistiques.
    pub stats: ChannelStats,
    /// Nom du canal (pour debug/dmesg).
    pub name: [u8; 32],
    /// Pointeur vers la structure matérielle privée (opaque, encodé en usize).
    hw_private: AtomicU64,
}

impl DmaChannel {
    const fn new_free(id: u32) -> Self {
        DmaChannel {
            id:           DmaChannelId(id),
            capabilities: DmaCapabilities::NONE,
            priority:     DmaPriority::Normal,
            state:        AtomicU32::new(ChannelState::Free as u32),
            registered:   AtomicBool::new(false),
            cpu_affinity: u8::MAX,
            _pad:         [0u8; 2],
            queue:        Mutex::new(ChannelQueue::new()),
            stats:        ChannelStats::new(),
            name:         [0u8; 32],
            hw_private:   AtomicU64::new(0),
        }
    }

    // ── État ─────────────────────────────────────────────────────────────────

    pub fn state(&self) -> ChannelState {
        match self.state.load(Ordering::Acquire) {
            0 => ChannelState::Free,
            1 => ChannelState::Idle,
            2 => ChannelState::Running,
            3 => ChannelState::Paused,
            4 => ChannelState::Error,
            5 => ChannelState::Terminating,
            _ => ChannelState::Error,
        }
    }

    pub fn set_state(&self, s: ChannelState) {
        self.state.store(s as u32, Ordering::Release);
    }

    pub fn is_available(&self) -> bool {
        self.registered.load(Ordering::Acquire)
            && matches!(self.state(), ChannelState::Idle)
    }

    // ── Capacités ────────────────────────────────────────────────────────────

    pub fn supports(&self, cap: DmaCapabilities) -> bool {
        self.capabilities.has(cap)
    }

    // ── Queue ─────────────────────────────────────────────────────────────────

    pub fn enqueue(&self, txn: DmaTransactionId) -> Result<(), DmaError> {
        let mut q = self.queue.lock();
        if q.is_full() { return Err(DmaError::OutOfMemory); }
        q.push(txn);
        self.stats.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn dequeue(&self) -> Option<DmaTransactionId> {
        self.queue.lock().pop()
    }

    pub fn queue_depth(&self) -> usize {
        self.queue.lock().len()
    }

    // ── Nom ──────────────────────────────────────────────────────────────────

    pub fn set_name(&mut self, name: &[u8]) {
        let len = name.len().min(31);
        self.name[..len].copy_from_slice(&name[..len]);
        self.name[len] = 0;
    }

    pub fn name_str(&self) -> &[u8] {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        &self.name[..end]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DES CANAUX
// ─────────────────────────────────────────────────────────────────────────────

pub struct DmaChannelManager {
    channels: [DmaChannel; MAX_DMA_CHANNELS],
    count:    AtomicU32,
    stats_total_submitted:  AtomicU64,
    stats_total_completed:  AtomicU64,
    stats_allocation_fails: AtomicU64,
}

// SAFETY: DmaChannelManager est protégé par les Mutex internes des canaux.
unsafe impl Sync for DmaChannelManager {}
unsafe impl Send for DmaChannelManager {}

impl DmaChannelManager {
    const fn new() -> Self {
        // Initialise les 128 canaux statiquement.
        macro_rules! ch { ($i:expr) => { DmaChannel::new_free($i) }; }
        DmaChannelManager {
            channels: [
                ch!(  0), ch!(  1), ch!(  2), ch!(  3), ch!(  4), ch!(  5), ch!(  6), ch!(  7),
                ch!(  8), ch!(  9), ch!( 10), ch!( 11), ch!( 12), ch!( 13), ch!( 14), ch!( 15),
                ch!( 16), ch!( 17), ch!( 18), ch!( 19), ch!( 20), ch!( 21), ch!( 22), ch!( 23),
                ch!( 24), ch!( 25), ch!( 26), ch!( 27), ch!( 28), ch!( 29), ch!( 30), ch!( 31),
                ch!( 32), ch!( 33), ch!( 34), ch!( 35), ch!( 36), ch!( 37), ch!( 38), ch!( 39),
                ch!( 40), ch!( 41), ch!( 42), ch!( 43), ch!( 44), ch!( 45), ch!( 46), ch!( 47),
                ch!( 48), ch!( 49), ch!( 50), ch!( 51), ch!( 52), ch!( 53), ch!( 54), ch!( 55),
                ch!( 56), ch!( 57), ch!( 58), ch!( 59), ch!( 60), ch!( 61), ch!( 62), ch!( 63),
                ch!( 64), ch!( 65), ch!( 66), ch!( 67), ch!( 68), ch!( 69), ch!( 70), ch!( 71),
                ch!( 72), ch!( 73), ch!( 74), ch!( 75), ch!( 76), ch!( 77), ch!( 78), ch!( 79),
                ch!( 80), ch!( 81), ch!( 82), ch!( 83), ch!( 84), ch!( 85), ch!( 86), ch!( 87),
                ch!( 88), ch!( 89), ch!( 90), ch!( 91), ch!( 92), ch!( 93), ch!( 94), ch!( 95),
                ch!( 96), ch!( 97), ch!( 98), ch!( 99), ch!(100), ch!(101), ch!(102), ch!(103),
                ch!(104), ch!(105), ch!(106), ch!(107), ch!(108), ch!(109), ch!(110), ch!(111),
                ch!(112), ch!(113), ch!(114), ch!(115), ch!(116), ch!(117), ch!(118), ch!(119),
                ch!(120), ch!(121), ch!(122), ch!(123), ch!(124), ch!(125), ch!(126), ch!(127),
            ],
            count:                    AtomicU32::new(0),
            stats_total_submitted:    AtomicU64::new(0),
            stats_total_completed:    AtomicU64::new(0),
            stats_allocation_fails:   AtomicU64::new(0),
        }
    }

    /// Enregistre un canal sur un slot libre.
    ///
    /// # Safety
    /// Appelé depuis l'init du driver DMA, avant le premier transfert.
    pub unsafe fn register_channel(
        &self,
        caps:     DmaCapabilities,
        priority: DmaPriority,
        name:     &[u8],
        affinity: u8,
        hw_priv:  u64,
    ) -> Result<DmaChannelId, DmaError> {
        let count = self.count.load(Ordering::Relaxed) as usize;
        if count >= MAX_DMA_CHANNELS { return Err(DmaError::NoChannel); }

        let ch_ptr = &self.channels[count] as *const DmaChannel as *mut DmaChannel;
        (*ch_ptr).capabilities = caps;
        (*ch_ptr).priority     = priority;
        (*ch_ptr).cpu_affinity = affinity;
        (*ch_ptr).set_name(name);
        (*ch_ptr).hw_private.store(hw_priv, Ordering::Release);
        (*ch_ptr).set_state(ChannelState::Idle);
        (*ch_ptr).registered.store(true, Ordering::Release);

        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(DmaChannelId(count as u32))
    }

    /// Alloue le premier canal disponible supportant `required_caps` et `min_priority`.
    pub fn alloc_channel(
        &self,
        required_caps: DmaCapabilities,
        min_priority:  DmaPriority,
    ) -> Option<DmaChannelId> {
        let count = self.count.load(Ordering::Relaxed) as usize;
        for i in 0..count {
            let ch = &self.channels[i];
            if ch.is_available()
                && ch.supports(required_caps)
                && ch.priority >= min_priority
            {
                ch.set_state(ChannelState::Running);
                return Some(ch.id);
            }
        }
        self.stats_allocation_fails.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Libère un canal après l'utilisation.
    pub fn free_channel(&self, id: DmaChannelId) {
        if (id.0 as usize) < MAX_DMA_CHANNELS {
            self.channels[id.0 as usize].set_state(ChannelState::Idle);
        }
    }

    /// Accès à un canal par ID.
    pub fn channel(&self, id: DmaChannelId) -> Option<&DmaChannel> {
        if (id.0 as usize) < MAX_DMA_CHANNELS {
            let ch = &self.channels[id.0 as usize];
            if ch.registered.load(Ordering::Acquire) { return Some(ch); }
        }
        None
    }

    pub fn channel_count(&self) -> usize { self.count.load(Ordering::Relaxed) as usize }

    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.stats_total_submitted.load(Ordering::Relaxed),
            self.stats_total_completed.load(Ordering::Relaxed),
            self.stats_allocation_fails.load(Ordering::Relaxed),
        )
    }
}

pub static DMA_CHANNELS: DmaChannelManager = DmaChannelManager::new();
