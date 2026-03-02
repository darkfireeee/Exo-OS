//! CryptoAuditLog — journal d'audit des opérations cryptographiques ExoFS (no_std).
//!
//! Les opérations critiques (génération/rotation/révocation de clé, chiffrement,
//! déchiffrement échoué) sont enregistrées dans un ring-buffer circulaire.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};

/// Capacité du ring-buffer d'audit.
const AUDIT_RING_CAPACITY: usize = 4096;

/// Types d'événements cryptographiques auditables.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoEvent {
    KeyGenerated         = 1,
    KeyLoaded            = 2,
    KeyRevoked           = 3,
    KeyRotationStarted   = 4,
    KeyRotationCompleted = 5,
    KeyRotationFailed    = 6,
    BlobEncrypted        = 7,
    BlobDecrypted        = 8,
    DecryptionFailed     = 9,
    NonceMaterialized    = 10,
    MasterKeyDerived     = 11,
    VolumeKeyWrapped     = 12,
    VolumeKeyUnwrapped   = 13,
    PolicyViolation      = 14,
}

/// Entrée d'audit (64 bytes, #[repr(C)]).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuditEntry {
    pub event:       u8,
    pub severity:    u8,    // 0=Info, 1=Warn, 2=Error, 3=Critical
    pub _pad:        [u8; 2],
    pub pid:         u32,
    pub tick:        u64,
    pub blob_id:     [u8; 32],  // BlobId associé (0 si N/A)
    pub extra_u64:   u64,       // Valeur contextuelle (key_id, error_code…)
    pub sequence:    u64,
}

const _: () = assert!(core::mem::size_of::<AuditEntry>() == 64);

/// Ring-buffer d'audit cryptographique.
pub static CRYPTO_AUDIT: CryptoAuditLog = CryptoAuditLog::new_const();

pub struct CryptoAuditLog {
    ring:      SpinLock<AuditRing>,
    next_seq:  AtomicU64,
    total:     AtomicU64,
    dropped:   AtomicU64,
}

struct AuditRing {
    entries: Vec<AuditEntry>,
    head:    usize,   // Prochain emplacement d'écriture.
    len:     usize,   // Nombre d'entrées valides.
}

impl CryptoAuditLog {
    pub const fn new_const() -> Self {
        Self {
            ring:     SpinLock::new(AuditRing {
                entries: Vec::new(),
                head:    0,
                len:     0,
            }),
            next_seq: AtomicU64::new(1),
            total:    AtomicU64::new(0),
            dropped:  AtomicU64::new(0),
        }
    }

    /// Initialise le ring-buffer (doit être appelé une fois).
    pub fn init(&self) -> Result<(), FsError> {
        let mut ring = self.ring.lock();
        if !ring.entries.is_empty() { return Ok(()); }
        ring.entries.try_reserve(AUDIT_RING_CAPACITY)
            .map_err(|_| FsError::OutOfMemory)?;
        ring.entries.resize(AUDIT_RING_CAPACITY, AuditEntry {
            event: 0, severity: 0, _pad: [0;2], pid: 0, tick: 0,
            blob_id: [0;32], extra_u64: 0, sequence: 0,
        });
        Ok(())
    }

    /// Enregistre un événement d'audit.
    pub fn record(
        &self,
        event: CryptoEvent,
        severity: u8,
        blob_id: Option<&BlobId>,
        extra: u64,
    ) {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let tick = crate::arch::time::read_ticks();
        let bid = blob_id.map(|b| b.as_bytes()).unwrap_or([0u8; 32]);

        let entry = AuditEntry {
            event:     event as u8,
            severity,
            _pad:      [0; 2],
            pid:       0, // TODO : récupérer le PID courant quand le scheduler sera disponible.
            tick,
            blob_id:   bid,
            extra_u64: extra,
            sequence:  seq,
        };

        let mut ring = self.ring.lock();
        if ring.entries.is_empty() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let idx = ring.head % AUDIT_RING_CAPACITY;
        ring.entries[idx] = entry;
        ring.head = (ring.head + 1) % AUDIT_RING_CAPACITY;
        if ring.len < AUDIT_RING_CAPACITY {
            ring.len += 1;
        }
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    /// Retourne les `n` dernières entrées d'audit.
    pub fn tail(&self, n: usize) -> Result<Vec<AuditEntry>, FsError> {
        let ring = self.ring.lock();
        if ring.entries.is_empty() { return Ok(Vec::new()); }

        let count = n.min(ring.len);
        let mut out = Vec::new();
        out.try_reserve(count).map_err(|_| FsError::OutOfMemory)?;

        let end = ring.head;
        for i in 0..count {
            let idx = (end + AUDIT_RING_CAPACITY - count + i) % AUDIT_RING_CAPACITY;
            out.push(ring.entries[idx]);
        }
        Ok(out)
    }

    /// Retourne les entrées avec severity >= `min_severity`.
    pub fn tail_filtered(
        &self,
        n: usize,
        min_severity: u8,
    ) -> Result<Vec<AuditEntry>, FsError> {
        let all = self.tail(n * 4)?; // Lire plus large pour filtrer.
        let mut out = Vec::new();
        for e in all {
            if e.severity >= min_severity {
                out.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                out.push(e);
                if out.len() >= n { break; }
            }
        }
        Ok(out)
    }

    pub fn total_events(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Raccourci : enregistre une violation de politique critique.
    pub fn policy_violation(&self, detail: u64) {
        self.record(CryptoEvent::PolicyViolation, 3, None, detail);
    }

    /// Raccourci : enregistre un échec de déchiffrement.
    pub fn decryption_failed(&self, blob_id: &BlobId, error_code: u64) {
        self.record(CryptoEvent::DecryptionFailed, 2, Some(blob_id), error_code);
    }
}
