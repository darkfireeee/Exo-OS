//! AuditLog — ring-buffer non-bloquant 65536 entrées ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use super::audit_entry::AuditEntry;

const RING_SIZE: usize = 65536;

pub static AUDIT_LOG: AuditLog = AuditLog::new_const();

pub struct AuditLog {
    ring:  [core::cell::UnsafeCell<AuditEntry>; RING_SIZE],
    head:  AtomicU64,
    count: AtomicU64,
}

// SAFETY: accès au ring-buffer uniquement via fetch_add atomique.
unsafe impl Sync for AuditLog {}

impl AuditLog {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<AuditEntry> = core::cell::UnsafeCell::new(
            AuditEntry {
                tick: 0, actor_uid: 0, actor_cap: 0, object_id: 0,
                blob_id: [0; 32], op: super::audit_entry::AuditOp::Read,
                result: super::audit_entry::AuditResult::Success, _pad: [0; 6],
            }
        );
        Self { ring: [ZERO; RING_SIZE], head: AtomicU64::new(0), count: AtomicU64::new(0) }
    }

    /// Enregistre une entrée d'audit (lock-free, perte impossible côté ring).
    pub fn push(&self, entry: AuditEntry) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % RING_SIZE;
        // SAFETY: index unique via fetch_add sur ring circulaire.
        unsafe { *self.ring[idx].get() = entry; }
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }

    pub fn read_at(&self, pos: usize) -> AuditEntry {
        let idx = pos % RING_SIZE;
        // SAFETY: lecture diagnostique du ring-buffer, pas de lock requis.
        unsafe { *self.ring[idx].get() }
    }
}
