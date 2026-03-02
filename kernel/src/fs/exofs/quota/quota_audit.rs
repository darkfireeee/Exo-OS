//! QuotaAuditLog — journal des événements de quota ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;

const AUDIT_RING_SIZE: usize = 2048;

/// Événement de quota auditable.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaEvent {
    SoftBreach  = 0x01,
    HardDenial  = 0x02,
    LimitSet    = 0x03,
    LimitReset  = 0x04,
    NamespaceAdded   = 0x05,
    NamespaceRemoved = 0x06,
}

/// Entrée d'audit de quota.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QuotaAuditEntry {
    pub tick:       u64,
    pub entity_id:  u64,
    pub current:    u64,
    pub limit:      u64,
    pub event:      QuotaEvent,
    pub _pad:       [u8; 7],
}

const _: () = assert!(core::mem::size_of::<QuotaAuditEntry>() == 40);

pub static QUOTA_AUDIT: QuotaAuditLog = QuotaAuditLog::new_const();

pub struct QuotaAuditLog {
    ring:  [core::cell::UnsafeCell<QuotaAuditEntry>; AUDIT_RING_SIZE],
    head:  AtomicU64,
    count: AtomicU64,
}

// SAFETY: QuotaAuditLog n'est accédé que via atomics et via SpinLock implicite du head.
unsafe impl Sync for QuotaAuditLog {}

impl QuotaAuditLog {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<QuotaAuditEntry> = core::cell::UnsafeCell::new(
            QuotaAuditEntry { tick: 0, entity_id: 0, current: 0, limit: 0, event: QuotaEvent::SoftBreach, _pad: [0; 7] }
        );
        Self {
            ring:  [ZERO; AUDIT_RING_SIZE],
            head:  AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn push_entry(&self, entry: QuotaAuditEntry) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % AUDIT_RING_SIZE;
        // SAFETY: Accès exclusif géré par fetch_add atomique sur le slot tournant.
        unsafe { *self.ring[idx].get() = entry; }
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_soft_breach(&self, entity_id: u64, current: u64, limit: u64) {
        self.push_entry(QuotaAuditEntry {
            tick: read_ticks(), entity_id, current, limit,
            event: QuotaEvent::SoftBreach, _pad: [0; 7],
        });
    }

    pub fn record_hard_denial(&self, entity_id: u64, current: u64, limit: u64) {
        self.push_entry(QuotaAuditEntry {
            tick: read_ticks(), entity_id, current, limit,
            event: QuotaEvent::HardDenial, _pad: [0; 7],
        });
    }

    pub fn record_limit_set(&self, entity_id: u64, limit: u64) {
        self.push_entry(QuotaAuditEntry {
            tick: read_ticks(), entity_id, current: 0, limit,
            event: QuotaEvent::LimitSet, _pad: [0; 7],
        });
    }

    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }

    /// Lecture pour inspection (dernier N éléments, dans l'ordre d'insertion).
    pub fn read_recent(&self, n: usize) -> alloc::vec::Vec<QuotaAuditEntry> {
        let total = self.count.load(Ordering::Relaxed) as usize;
        let head  = self.head.load(Ordering::Relaxed) as usize;
        let n     = n.min(AUDIT_RING_SIZE).min(total);
        let mut out = alloc::vec::Vec::new();
        let _ = out.try_reserve(n);
        for i in 0..n {
            let idx = (head + AUDIT_RING_SIZE - n + i) % AUDIT_RING_SIZE;
            // SAFETY: Lecture sans lock acceptable ici — ring-buffer en lecture seule pour diagnostic.
            let entry = unsafe { *self.ring[idx].get() };
            out.push(entry);
        }
        out
    }
}
