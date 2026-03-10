// kernel/src/memory/dma/channels/priority.rs
//
// Ordonnancement par priorité des canaux DMA — files RT vs best-effort.
//
// Architecture :
//   - 4 niveaux de priorité : Low, Normal, High, Realtime (depuis DmaPriority).
//   - Une file circulaire dédiée par niveau (CHANNEL_QUEUE_DEPTH entrées/niveau).
//   - Le scheduler DMA dépile toujours dans l'ordre décroissant de priorité.
//   - Les transactions RT ont une deadline (time-to-live en passes de scheduler).
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::dma::core::types::{DmaChannelId, DmaTransactionId, DmaPriority, DmaError};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de niveaux de priorité (Low=0, Normal=1, High=2, Realtime=3).
pub const PRIORITY_LEVELS: usize = 4;

/// Profondeur de chaque file de priorité (en transactions).
pub const PRIORITY_QUEUE_DEPTH: usize = 64;

/// Nombre de passes avant qu'une transaction best-effort soit promue
/// pour éviter la famine (starvation prevention).
pub const STARVATION_THRESHOLD: u32 = 32;

// ─────────────────────────────────────────────────────────────────────────────
// ENTRÉE DE FILE DE PRIORITÉ
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans une file de priorité.
#[derive(Copy, Clone)]
struct PrioEntry {
    /// Transaction DMA associée.
    txn_id:     DmaTransactionId,
    /// Canal DMA demandeur.
    channel_id: DmaChannelId,
    /// Nombre de passes de scheduler écoulées sans être dépilée (anti-famine).
    age_passes: u32,
}

impl PrioEntry {
    const EMPTY: Self = PrioEntry {
        txn_id:     DmaTransactionId::INVALID,
        channel_id: DmaChannelId(u32::MAX),
        age_passes: 0,
    };

    fn is_valid(self) -> bool {
        self.txn_id.0 != DmaTransactionId::INVALID.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FILE CIRCULAIRE PAR NIVEAU
// ─────────────────────────────────────────────────────────────────────────────

struct PrioQueue {
    entries: [PrioEntry; PRIORITY_QUEUE_DEPTH],
    head:    usize,
    tail:    usize,
    count:   usize,
}

impl PrioQueue {
    const fn new() -> Self {
        PrioQueue {
            entries: [PrioEntry::EMPTY; PRIORITY_QUEUE_DEPTH],
            head:    0,
            tail:    0,
            count:   0,
        }
    }

    fn push(&mut self, entry: PrioEntry) -> bool {
        if self.count >= PRIORITY_QUEUE_DEPTH { return false; }
        self.entries[self.tail] = entry;
        self.tail = (self.tail + 1) % PRIORITY_QUEUE_DEPTH;
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<PrioEntry> {
        if self.count == 0 { return None; }
        let entry = self.entries[self.head];
        self.entries[self.head] = PrioEntry::EMPTY;
        self.head = (self.head + 1) % PRIORITY_QUEUE_DEPTH;
        self.count -= 1;
        Some(entry)
    }

    fn peek(&self) -> Option<&PrioEntry> {
        if self.count == 0 { return None; }
        Some(&self.entries[self.head])
    }

    fn len(&self) -> usize { self.count }
    fn is_full(&self)  -> bool { self.count >= PRIORITY_QUEUE_DEPTH }
    fn is_empty(&self) -> bool { self.count == 0 }

    /// Incrémente l'âge de toutes les entrées (anti-famine).
    fn age_all(&mut self) {
        let mut idx = self.head;
        for _ in 0..self.count {
            self.entries[idx].age_passes =
                self.entries[idx].age_passes.saturating_add(1);
            idx = (idx + 1) % PRIORITY_QUEUE_DEPTH;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SCHEDULER MULTI-PRIORITÉS
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du scheduler de priorité.
pub struct PrioritySchedulerStats {
    pub enqueued:   [AtomicU64; PRIORITY_LEVELS],
    pub dequeued:   [AtomicU64; PRIORITY_LEVELS],
    pub promoted:   AtomicU64,
    pub full_drops: AtomicU64,
}

impl PrioritySchedulerStats {
    const fn new() -> Self {
        PrioritySchedulerStats {
            enqueued:   [AtomicU64::new(0), AtomicU64::new(0),
                         AtomicU64::new(0), AtomicU64::new(0)],
            dequeued:   [AtomicU64::new(0), AtomicU64::new(0),
                         AtomicU64::new(0), AtomicU64::new(0)],
            promoted:   AtomicU64::new(0),
            full_drops: AtomicU64::new(0),
        }
    }
}

struct PrioritySchedulerInner {
    queues: [PrioQueue; PRIORITY_LEVELS],
    /// Nombre de passes depuis la dernière anti-famine sweep.
    pass_counter: u32,
}

impl PrioritySchedulerInner {
    const fn new() -> Self {
        PrioritySchedulerInner {
            queues:       [
                PrioQueue::new(), PrioQueue::new(),
                PrioQueue::new(), PrioQueue::new(),
            ],
            pass_counter: 0,
        }
    }

    fn prio_index(p: DmaPriority) -> usize {
        match p {
            DmaPriority::Low      => 0,
            DmaPriority::Normal   => 1,
            DmaPriority::High     => 2,
            DmaPriority::Realtime => 3,
        }
    }
}

/// Scheduler multi-priorité pour transactions DMA.
///
/// Thread-safe via spinlock. Toutes les opérations sont O(1) sauf
/// le sweep anti-famine qui est O(PRIORITY_QUEUE_DEPTH * PRIORITY_LEVELS).
pub struct PriorityScheduler {
    inner: Mutex<PrioritySchedulerInner>,
    pub stats: PrioritySchedulerStats,
}

// SAFETY: PriorityScheduler est protégé par un Mutex.
unsafe impl Sync for PriorityScheduler {}
unsafe impl Send for PriorityScheduler {}

impl PriorityScheduler {
    pub const fn new() -> Self {
        PriorityScheduler {
            inner: Mutex::new(PrioritySchedulerInner::new()),
            stats: PrioritySchedulerStats::new(),
        }
    }

    /// Enfile une transaction avec la priorité donnée.
    ///
    /// Retourne `Err(DmaError::OutOfMemory)` si la file de ce niveau est pleine.
    pub fn enqueue(
        &self,
        txn_id:     DmaTransactionId,
        channel_id: DmaChannelId,
        priority:   DmaPriority,
    ) -> Result<(), DmaError> {
        let level = PrioritySchedulerInner::prio_index(priority);
        let entry = PrioEntry { txn_id, channel_id, age_passes: 0 };

        let mut inner = self.inner.lock();
        if !inner.queues[level].push(entry) {
            self.stats.full_drops.fetch_add(1, Ordering::Relaxed);
            return Err(DmaError::OutOfMemory);
        }
        self.stats.enqueued[level].fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Défile la prochaine transaction selon la priorité la plus haute.
    ///
    /// Vérifie d'abord les entrées vieillies (anti-famine) pour les niveaux basses,
    /// puis dépile de la file de plus haute priorité non vide.
    ///
    /// Retourne `None` si aucune transaction n'est en attente.
    pub fn dequeue(&self) -> Option<(DmaTransactionId, DmaChannelId, DmaPriority)> {
        let mut inner = self.inner.lock();

        // Anti-famine : toutes les N passes, promouvoir les anciennes entrées.
        inner.pass_counter = inner.pass_counter.wrapping_add(1);
        if inner.pass_counter % STARVATION_THRESHOLD == 0 {
            self.check_starvation(&mut inner);
        }

        // Dépiler dans l'ordre décroissant de priorité (3 = Realtime → 0 = Low).
        for level in (0..PRIORITY_LEVELS).rev() {
            if let Some(entry) = inner.queues[level].pop() {
                let prio = match level {
                    3 => DmaPriority::Realtime,
                    2 => DmaPriority::High,
                    1 => DmaPriority::Normal,
                    _ => DmaPriority::Low,
                };
                self.stats.dequeued[level].fetch_add(1, Ordering::Relaxed);
                return Some((entry.txn_id, entry.channel_id, prio));
            }
        }
        None
    }

    /// Retourne le nombre total de transactions en attente.
    pub fn total_pending(&self) -> usize {
        let inner = self.inner.lock();
        inner.queues.iter().map(|q| q.len()).sum()
    }

    /// Retourne le nombre de transactions en attente par niveau.
    pub fn pending_by_level(&self) -> [usize; PRIORITY_LEVELS] {
        let inner = self.inner.lock();
        [
            inner.queues[0].len(),
            inner.queues[1].len(),
            inner.queues[2].len(),
            inner.queues[3].len(),
        ]
    }

    // ── Anti-famine ─────────────────────────────────────────────────────────

    /// Vérifie si des entrées basse priorité sont trop vieilles et les promeut.
    fn check_starvation(&self, inner: &mut PrioritySchedulerInner) {
        // Pour chaque niveau < Realtime, chercher des entrées âgées.
        for src_level in 0..PRIORITY_LEVELS - 1 {
            let dst_level = (src_level + 1).min(PRIORITY_LEVELS - 1);
            // Âge de toutes les entrées du niveau source.
            inner.queues[src_level].age_all();

            // Promouvoir les entrées qui ont dépassé le seuil.
            let src_len = inner.queues[src_level].len();
            let mut promoted_count = 0;
            for _ in 0..src_len {
                let aged = match inner.queues[src_level].peek() {
                    Some(e) => e.age_passes >= STARVATION_THRESHOLD,
                    None    => false,
                };
                if aged {
                    if let Some(mut entry) = inner.queues[src_level].pop() {
                        entry.age_passes = 0;
                        if inner.queues[dst_level].push(entry) {
                            promoted_count += 1;
                        } else {
                            // File destination pleine — remettre à la source.
                            inner.queues[src_level].push(entry);
                        }
                    }
                } else {
                    break; // Entrées restantes non âgées, FIFO garanti.
                }
            }
            if promoted_count > 0 {
                self.stats.promoted.fetch_add(promoted_count as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Instance globale du scheduler de priorité DMA.
pub static DMA_PRIORITY_SCHEDULER: PriorityScheduler = PriorityScheduler::new();
