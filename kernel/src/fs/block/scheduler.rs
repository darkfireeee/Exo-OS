// kernel/src/fs/block/scheduler.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// I/O SCHEDULER — Ordonnancement bloc (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Réordonne les requêtes I/O pour maximiser le débit tout en évitant la
// famine des requêtes anciennes.
//
// Algorithme : Deadline I/O Scheduler (simplifié)
//   • Deux files séparées : lecture (deadline = 500 ms) et écriture (5 s).
//   • File de dispatching ordered by secteur pour exploiter la localité.
//   • Expiration : si une requête dépasse son deadline, elle passe en priorité.
//   • `schedule_dispatch()` retourne la prochaine Bio à servir.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrd;

use crate::fs::core::types::FsError;
use crate::fs::block::bio::{Bio, BioOp};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Deadlines (en ticks monotones — 1 tick = 1 ms)
// ─────────────────────────────────────────────────────────────────────────────
const READ_DEADLINE_TICKS:  u64 = 500;
const WRITE_DEADLINE_TICKS: u64 = 5_000;

// ─────────────────────────────────────────────────────────────────────────────
// Tick monotone global (incrémenté par l'horloge système)
// ─────────────────────────────────────────────────────────────────────────────
pub static SCHED_TICK: AtomicU64 = AtomicU64::new(0);

pub fn tick() -> u64 { SCHED_TICK.load(Ordering::Relaxed) }
pub fn advance_tick() { SCHED_TICK.fetch_add(1, Ordering::Relaxed); }

// ─────────────────────────────────────────────────────────────────────────────
// ScheduledBio — Bio avec deadline et secteur clé
// ─────────────────────────────────────────────────────────────────────────────

struct ScheduledBio {
    bio:      Bio,
    deadline: u64,
    sector:   u64,
}

// Min-heap par secteur (Ord inverse car BinaryHeap est max-heap).
impl Ord for ScheduledBio {
    fn cmp(&self, other: &Self) -> CmpOrd {
        other.sector.cmp(&self.sector)
    }
}
impl PartialOrd for ScheduledBio { fn partial_cmp(&self, other: &Self) -> Option<CmpOrd> { Some(self.cmp(other)) } }
impl PartialEq  for ScheduledBio { fn eq(&self, other: &Self) -> bool { self.sector == other.sector } }
impl Eq         for ScheduledBio {}

// ─────────────────────────────────────────────────────────────────────────────
// DeadlineScheduler
// ─────────────────────────────────────────────────────────────────────────────

pub struct DeadlineScheduler {
    read_queue:  SpinLock<BinaryHeap<ScheduledBio>>,
    write_queue: SpinLock<BinaryHeap<ScheduledBio>>,
    /// Pointeur de dispatching (secteur courant de la tête de lecture fictive).
    dispatch_head: AtomicU64,
    /// Compteurs.
    pub enqueued:    AtomicU64,
    pub dispatched:  AtomicU64,
    pub expired:     AtomicU64,
}

impl DeadlineScheduler {
    pub const fn new() -> Self {
        Self {
            read_queue:   SpinLock::new(BinaryHeap::new()),
            write_queue:  SpinLock::new(BinaryHeap::new()),
            dispatch_head: AtomicU64::new(0),
            enqueued:     AtomicU64::new(0),
            dispatched:   AtomicU64::new(0),
            expired:      AtomicU64::new(0),
        }
    }

    /// Enqueue une Bio dans le scheduler.
    pub fn enqueue(&self, bio: Bio) {
        let now = tick();
        let (deadline, is_read) = match bio.op {
            BioOp::Read  => (now + READ_DEADLINE_TICKS,  true),
            _            => (now + WRITE_DEADLINE_TICKS, false),
        };
        let sector = bio.sector;
        let sbio   = ScheduledBio { bio, deadline, sector };
        if is_read {
            self.read_queue.lock().push(sbio);
        } else {
            self.write_queue.lock().push(sbio);
        }
        self.enqueued.fetch_add(1, Ordering::Relaxed);
    }

    /// Dispatch la prochaine Bio.
    ///
    /// Priorité :
    ///   1. Requêtes expirées (deadline dépassé) — round-robin read/write.
    ///   2. Requête read avec secteur le plus proche de `dispatch_head`.
    ///   3. Requête write.
    pub fn dispatch_next(&self) -> Option<Bio> {
        let now = tick();

        // 1. Vérifier les requêtes expirées.
        {
            let mut rq = self.read_queue.lock();
            // On scanne la BinaryHeap (pas d'accès direct aux éléments…)
            // On utilise un Vec temporaire pour trouver l'expirée.
            let items: Vec<_> = rq.drain().collect();
            let mut expired_bio = None;
            let mut rest = Vec::new();
            for item in items {
                if expired_bio.is_none() && item.deadline <= now {
                    expired_bio = Some(item.bio);
                    self.expired.fetch_add(1, Ordering::Relaxed);
                } else {
                    rest.push(item);
                }
            }
            for item in rest { rq.push(item); }
            if let Some(bio) = expired_bio {
                self.dispatch_head.store(bio.sector, Ordering::Relaxed);
                self.dispatched.fetch_add(1, Ordering::Relaxed);
                return Some(bio);
            }
        }

        // 2. Dispatch par secteur (read).
        if let Some(sbio) = self.read_queue.lock().pop() {
            self.dispatch_head.store(sbio.sector, Ordering::Relaxed);
            self.dispatched.fetch_add(1, Ordering::Relaxed);
            return Some(sbio.bio);
        }

        // 3. Write queue.
        if let Some(sbio) = self.write_queue.lock().pop() {
            self.dispatch_head.store(sbio.sector, Ordering::Relaxed);
            self.dispatched.fetch_add(1, Ordering::Relaxed);
            return Some(sbio.bio);
        }

        None
    }

    pub fn pending(&self) -> usize {
        self.read_queue.lock().len() + self.write_queue.lock().len()
    }
}

pub static IO_SCHEDULER: DeadlineScheduler = DeadlineScheduler::new();

/// Enqueue une Bio dans le scheduler global.
pub fn schedule_io(bio: Bio) {
    IO_SCHEDULER.enqueue(bio);
}

/// Dispatch une batch d'au plus `max` Bio depuis le scheduler.
pub fn schedule_dispatch(max: usize) -> Vec<Bio> {
    let mut out = Vec::new();
    for _ in 0..max {
        match IO_SCHEDULER.dispatch_next() {
            Some(bio) => out.push(bio),
            None      => break,
        }
    }
    out
}
