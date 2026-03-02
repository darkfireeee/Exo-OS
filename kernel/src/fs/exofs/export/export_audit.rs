//! export_audit.rs — Journal d'audit des opérations d'export/import (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;

const EXPORT_AUDIT_RING: usize = 512;

pub static EXPORT_AUDIT: ExportAuditLog = ExportAuditLog::new_const();

/// Type d'événement d'export.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportEvent {
    ExportStarted       = 0,
    ExportCompleted     = 1,
    ExportFailed        = 2,
    ImportStarted       = 3,
    ImportCompleted     = 4,
    ImportFailed        = 5,
    BlobExported        = 6,
    BlobImported        = 7,
    VerificationFailed  = 8,
    PartialExport       = 9,
}

/// Entrée d'audit.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExportAuditEntry {
    pub tick:       u64,
    pub event:      ExportEvent,
    pub _pad:       [u8; 7],
    pub blob_id:    [u8; 32],
    pub n_blobs:    u64,
    pub bytes:      u64,
    pub session_id: u32,
    pub error_code: i32,
}

const _: () = assert!(core::mem::size_of::<ExportAuditEntry>() == 72);

pub struct ExportAuditLog {
    ring:    [core::cell::UnsafeCell<ExportAuditEntry>; EXPORT_AUDIT_RING],
    head:    AtomicU64,
}

// SAFETY: index tournant atomique.
unsafe impl Sync for ExportAuditLog {}

impl ExportAuditLog {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<ExportAuditEntry> = core::cell::UnsafeCell::new(
            ExportAuditEntry {
                tick: 0, event: ExportEvent::ExportStarted, _pad: [0;7],
                blob_id: [0;32], n_blobs: 0, bytes: 0, session_id: 0, error_code: 0,
            }
        );
        Self { ring: [ZERO; EXPORT_AUDIT_RING], head: AtomicU64::new(0) }
    }

    pub fn push(&self, event: ExportEvent, blob_id: [u8;32], n_blobs: u64, bytes: u64, session_id: u32, error_code: i32) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % EXPORT_AUDIT_RING;
        let e = ExportAuditEntry {
            tick: read_ticks(), event, _pad: [0;7],
            blob_id, n_blobs, bytes, session_id, error_code,
        };
        // SAFETY: accès exclusif par slot tournant.
        unsafe { *self.ring[idx].get() = e; }
    }

    pub fn total_ops(&self) -> u64 { self.head.load(Ordering::Relaxed) }
}
