//! RecoveryAudit — audit des opérations de récupération ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;

const AUDIT_RING_SIZE: usize = 512;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAuditEvent {
    SlotValidated    = 0x01,
    SlotInvalid      = 0x02,
    EpochReplayed    = 0x03,
    OrphanFound      = 0x04,
    OrphanRepaired   = 0x05,
    ChecksumMismatch = 0x06,
    MagicMismatch    = 0x07,
    SuperblockOk     = 0x08,
    SuperblockBad    = 0x09,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RecoveryAuditEntry {
    pub tick:  u64,
    pub event: RecoveryAuditEvent,
    pub _pad:  [u8; 3],
    pub lba:   u64,
    pub data:  u64,
}

const _: () = assert!(core::mem::size_of::<RecoveryAuditEntry>() == 28);

pub static RECOVERY_AUDIT: RecoveryAuditLog = RecoveryAuditLog::new_const();

pub struct RecoveryAuditLog {
    ring:  [core::cell::UnsafeCell<RecoveryAuditEntry>; AUDIT_RING_SIZE],
    head:  AtomicU64,
    count: AtomicU64,
}

// SAFETY: ring-buffer atomique sur head.
unsafe impl Sync for RecoveryAuditLog {}

impl RecoveryAuditLog {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<RecoveryAuditEntry> =
            core::cell::UnsafeCell::new(RecoveryAuditEntry {
                tick: 0, event: RecoveryAuditEvent::SlotValidated,
                _pad: [0; 3], lba: 0, data: 0,
            });
        Self { ring: [ZERO; AUDIT_RING_SIZE], head: AtomicU64::new(0), count: AtomicU64::new(0) }
    }

    pub fn record(&self, event: RecoveryAuditEvent, lba: u64, data: u64) {
        let entry = RecoveryAuditEntry { tick: read_ticks(), event, _pad: [0; 3], lba, data };
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % AUDIT_RING_SIZE;
        // SAFETY: Slot tournant géré par fetch_add atomique.
        unsafe { *self.ring[idx].get() = entry; }
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }

    pub fn read_recent(&self, n: usize) -> alloc::vec::Vec<RecoveryAuditEntry> {
        let total = self.count.load(Ordering::Relaxed) as usize;
        let head  = self.head.load(Ordering::Relaxed) as usize;
        let n     = n.min(AUDIT_RING_SIZE).min(total);
        let mut out = alloc::vec::Vec::new();
        let _ = out.try_reserve(n);
        for i in 0..n {
            let idx = (head + AUDIT_RING_SIZE - n + i) % AUDIT_RING_SIZE;
            // SAFETY: Lecture diagnostique.
            let e = unsafe { *self.ring[idx].get() };
            out.push(e);
        }
        out
    }
}
