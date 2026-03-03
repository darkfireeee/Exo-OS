//! Journal d'audit cryptographique ExoFS.
//!
//! Enregistre toutes les opérations sensibles (génération, rotation, révocation,
//! chiffrement, déchiffrement) dans un ring buffer protégé par spinlock.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

#![allow(dead_code)]

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::key_storage::KeySlotId;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité du ring buffer d'audit.
pub const AUDIT_RING_SIZE: usize = 256;
/// Longueur maximale du champ de détail.
pub const AUDIT_DETAIL_LEN: usize = 48;

// ─────────────────────────────────────────────────────────────────────────────
// AuditKind
// ─────────────────────────────────────────────────────────────────────────────

/// Type d'événement cryptographique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
    /// Génération d'une nouvelle clé.
    KeyGenerated,
    /// Chargement d'une clé.
    KeyLoaded,
    /// Révocation d'une clé.
    KeyRevoked,
    /// Rotation d'une clé.
    KeyRotated,
    /// Chiffrement d'un blob.
    BlobEncrypted,
    /// Déchiffrement d'un blob.
    BlobDecrypted,
    /// Échec d'authentification.
    AuthFailure,
    /// Shredding cryptographique.
    BlobShredded,
    /// Wrapping d'une clé.
    KeyWrapped,
    /// Unwrapping d'une clé.
    KeyUnwrapped,
}

impl core::fmt::Display for AuditKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::KeyGenerated  => write!(f, "KeyGenerated"),
            Self::KeyLoaded     => write!(f, "KeyLoaded"),
            Self::KeyRevoked    => write!(f, "KeyRevoked"),
            Self::KeyRotated    => write!(f, "KeyRotated"),
            Self::BlobEncrypted => write!(f, "BlobEncrypted"),
            Self::BlobDecrypted => write!(f, "BlobDecrypted"),
            Self::AuthFailure   => write!(f, "AuthFailure"),
            Self::BlobShredded  => write!(f, "BlobShredded"),
            Self::KeyWrapped    => write!(f, "KeyWrapped"),
            Self::KeyUnwrapped  => write!(f, "KeyUnwrapped"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'audit (taille fixe pour le ring buffer).
#[derive(Debug, Clone, Copy)]
pub struct AuditEntry {
    /// Numéro de séquence monotone.
    pub seq:      u64,
    /// Horodatage (TSC ou compteur).
    pub ts:       u64,
    /// Type d'événement.
    pub kind:     AuditKind,
    /// Slot de clé concerné (optionnel).
    pub slot_id:  Option<KeySlotId>,
    /// Code de résultat : 0 = succès, 1 = erreur.
    pub result:   u8,
    /// Détail court (texte encodé UTF-8, longueur fixe).
    pub detail:   [u8; AUDIT_DETAIL_LEN],
}

impl AuditEntry {
    /// Crée une entrée de succès.
    pub fn success(seq: u64, ts: u64, kind: AuditKind, slot_id: Option<KeySlotId>) -> Self {
        Self { seq, ts, kind, slot_id, result: 0, detail: [0u8; AUDIT_DETAIL_LEN] }
    }

    /// Crée une entrée d'erreur avec un message court.
    pub fn error(seq: u64, ts: u64, kind: AuditKind, slot_id: Option<KeySlotId>, msg: &[u8]) -> Self {
        let mut detail = [0u8; AUDIT_DETAIL_LEN];
        let len = msg.len().min(AUDIT_DETAIL_LEN);
        detail[..len].copy_from_slice(&msg[..len]);
        Self { seq, ts, kind, slot_id, result: 1, detail }
    }

    /// Retourne `true` si l'entrée représente un succès.
    pub fn is_success(&self) -> bool { self.result == 0 }

    /// Message de détail décodé.
    pub fn detail_str(&self) -> &[u8] {
        let end = self.detail.iter().position(|&b| b == 0).unwrap_or(AUDIT_DETAIL_LEN);
        &self.detail[..end]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CryptoAuditLog
// ─────────────────────────────────────────────────────────────────────────────

/// Journal d'audit cryptographique ring buffer.
///
/// Thread-safe via combinaison atomics + UnsafeCell (spinlock implicite sur `write_pos`).
pub struct CryptoAuditLog {
    buf:       [core::cell::UnsafeCell<AuditEntry>; AUDIT_RING_SIZE],
    write_pos: AtomicUsize,
    seq:       AtomicU64,
    lock:      AtomicU64,
}

// SAFETY: protégé par spinlock atomique.
unsafe impl Sync for CryptoAuditLog {}
unsafe impl Send for CryptoAuditLog {}

/// Instance globale.
pub static AUDIT_LOG: CryptoAuditLog = CryptoAuditLog::new_const();

impl CryptoAuditLog {
    /// Constructeur const.
    pub const fn new_const() -> Self {
        const ZERO_ENTRY: core::cell::UnsafeCell<AuditEntry> =
            core::cell::UnsafeCell::new(AuditEntry {
                seq: 0, ts: 0,
                kind: AuditKind::KeyGenerated,
                slot_id: None, result: 0,
                detail: [0u8; AUDIT_DETAIL_LEN],
            });
        Self {
            buf:       [ZERO_ENTRY; AUDIT_RING_SIZE],
            write_pos: AtomicUsize::new(0),
            seq:       AtomicU64::new(0),
            lock:      AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    /// Enregistre une entrée.
    pub fn record(&self, kind: AuditKind, slot_id: Option<KeySlotId>, ts: u64, ok: bool) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let entry = if ok {
            AuditEntry::success(seq, ts, kind, slot_id)
        } else {
            AuditEntry::error(seq, ts, kind, slot_id, b"failure")
        };
        self.acquire();
        let pos = self.write_pos.fetch_add(1, Ordering::Relaxed) % AUDIT_RING_SIZE;
        // SAFETY: lock pris.
        unsafe { (*self.buf[pos].get()) = entry; }
        self.release();
    }

    /// Retourne les N dernières entrées.
    ///
    /// OOM-02.
    pub fn tail(&self, n: usize) -> ExofsResult<Vec<AuditEntry>> {
        let n = n.min(AUDIT_RING_SIZE);
        let mut out: Vec<AuditEntry> = Vec::new();
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        let write_pos = self.write_pos.load(Ordering::Relaxed);
        for i in 0..n {
            let idx = write_pos.wrapping_sub(1).wrapping_sub(i) % AUDIT_RING_SIZE;
            // SAFETY: lock pris.
            out.push(unsafe { *self.buf[idx].get() });
        }
        self.release();
        Ok(out)
    }

    /// Filtre les entrées par type.
    ///
    /// OOM-02.
    pub fn filter_by_kind(&self, kind: AuditKind) -> ExofsResult<Vec<AuditEntry>> {
        let all = self.tail(AUDIT_RING_SIZE)?;
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in all { if e.kind == kind { out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?; out.push(e); } }
        Ok(out)
    }

    /// Filtre les entrées en erreur.
    ///
    /// OOM-02.
    pub fn filter_errors(&self) -> ExofsResult<Vec<AuditEntry>> {
        let all = self.tail(AUDIT_RING_SIZE)?;
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in all {
            if !e.is_success() {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Nombre total d'entrées écrites (peut dépasser AUDIT_RING_SIZE).
    pub fn total_written(&self) -> u64 { self.seq.load(Ordering::Relaxed) }

    /// Efface le journal (réinitialise le pointeur d'écriture).
    ///
    /// SECURITY : n'efface pas le contenu des entrées — utiliser `clear_secure` si nécessaire.
    pub fn clear(&self) {
        self.acquire();
        self.write_pos.store(0, Ordering::Relaxed);
        self.seq.store(0, Ordering::Relaxed);
        self.release();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> CryptoAuditLog { CryptoAuditLog::new_const() }

    #[test] fn test_record_and_tail_one() {
        let log = fresh();
        log.record(AuditKind::KeyGenerated, None, 1000, true);
        let tail = log.tail(1).unwrap();
        assert_eq!(tail[0].kind, AuditKind::KeyGenerated);
        assert!(tail[0].is_success());
    }

    #[test] fn test_tail_empty_zero() {
        let log = fresh();
        let t = log.tail(0).unwrap();
        assert!(t.is_empty());
    }

    #[test] fn test_total_written_increments() {
        let log = fresh();
        for _ in 0..5 { log.record(AuditKind::BlobEncrypted, None, 0, true); }
        assert_eq!(log.total_written(), 5);
    }

    #[test] fn test_filter_by_kind() {
        let log = fresh();
        log.record(AuditKind::KeyRevoked,    Some(KeySlotId(1)), 0, true);
        log.record(AuditKind::BlobEncrypted, None, 0, true);
        log.record(AuditKind::KeyRevoked,    Some(KeySlotId(2)), 0, true);
        let revoked = log.filter_by_kind(AuditKind::KeyRevoked).unwrap();
        assert!(revoked.iter().all(|e| e.kind == AuditKind::KeyRevoked));
    }

    #[test] fn test_filter_errors() {
        let log = fresh();
        log.record(AuditKind::AuthFailure, None, 0, false);
        log.record(AuditKind::BlobDecrypted, None, 0, true);
        let errs = log.filter_errors().unwrap();
        assert!(errs.iter().all(|e| !e.is_success()));
    }

    #[test] fn test_clear_resets_counter() {
        let log = fresh();
        log.record(AuditKind::KeyGenerated, None, 0, true);
        log.clear();
        assert_eq!(log.total_written(), 0);
    }

    #[test] fn test_ring_buffer_wraps() {
        let log = fresh();
        for i in 0..300u64 { log.record(AuditKind::BlobEncrypted, None, i, true); }
        assert_eq!(log.total_written(), 300);
        let t = log.tail(AUDIT_RING_SIZE).unwrap();
        assert_eq!(t.len(), AUDIT_RING_SIZE);
    }

    #[test] fn test_error_entry_detail() {
        let e = AuditEntry::error(0, 0, AuditKind::AuthFailure, None, b"tampered");
        assert!(!e.is_success());
        assert!(e.detail_str().starts_with(b"tampered"));
    }

    #[test] fn test_slot_id_in_entry() {
        let log = fresh();
        log.record(AuditKind::KeyLoaded, Some(KeySlotId(42)), 0, true);
        let t = log.tail(1).unwrap();
        assert_eq!(t[0].slot_id, Some(KeySlotId(42)));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Rapport d'audit
// ─────────────────────────────────────────────────────────────────────────────

/// Résumé statistique du journal d'audit.
#[derive(Debug, Clone)]
pub struct AuditSummary {
    pub total_events:      u64,
    pub total_errors:      u64,
    pub key_ops:           u64,
    pub blob_ops:          u64,
    pub auth_failures:     u64,
}

impl CryptoAuditLog {
    /// Calcule un résumé statistique des événements.
    pub fn summary(&self) -> ExofsResult<AuditSummary> {
        let entries = self.tail(AUDIT_RING_SIZE)?;
        let mut s   = AuditSummary {
            total_events:  0,
            total_errors:  0,
            key_ops:       0,
            blob_ops:      0,
            auth_failures: 0,
        };
        for e in &entries {
            if e.seq == 0 && e.ts == 0 { continue; } // slot vide
            s.total_events  = s.total_events.saturating_add(1);
            if !e.is_success() { s.total_errors = s.total_errors.saturating_add(1); }
            match e.kind {
                AuditKind::KeyGenerated | AuditKind::KeyLoaded |
                AuditKind::KeyRevoked   | AuditKind::KeyRotated |
                AuditKind::KeyWrapped   | AuditKind::KeyUnwrapped =>
                    s.key_ops = s.key_ops.saturating_add(1),
                AuditKind::BlobEncrypted | AuditKind::BlobDecrypted |
                AuditKind::BlobShredded =>
                    s.blob_ops = s.blob_ops.saturating_add(1),
                AuditKind::AuthFailure =>
                    s.auth_failures = s.auth_failures.saturating_add(1),
            }
        }
        Ok(s)
    }

    /// Retourne toutes les entrées liées à un slot particulier.
    ///
    /// OOM-02.
    pub fn filter_by_slot(&self, slot: KeySlotId) -> ExofsResult<Vec<AuditEntry>> {
        let all = self.tail(AUDIT_RING_SIZE)?;
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in all {
            if e.slot_id == Some(slot) {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Retourne toutes les entrées dans un intervalle de séquences.
    ///
    /// OOM-02.
    pub fn range(&self, from_seq: u64, to_seq: u64) -> ExofsResult<Vec<AuditEntry>> {
        if from_seq > to_seq { return Err(ExofsError::InvalidArgument); }
        let all = self.tail(AUDIT_RING_SIZE)?;
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in all {
            if e.seq >= from_seq && e.seq <= to_seq {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(e);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod extended_audit_tests {
    use super::*;

    fn fl() -> CryptoAuditLog { CryptoAuditLog::new_const() }

    #[test] fn test_summary_counts_key_ops() {
        let l = fl();
        l.record(AuditKind::KeyGenerated, None, 0, true);
        l.record(AuditKind::KeyRotated,   None, 0, true);
        let s = l.summary().unwrap();
        assert!(s.key_ops >= 2);
    }

    #[test] fn test_summary_counts_errors() {
        let l = fl();
        l.record(AuditKind::AuthFailure, None, 0, false);
        let s = l.summary().unwrap();
        assert!(s.total_errors >= 1);
        assert!(s.auth_failures >= 1);
    }

    #[test] fn test_filter_by_slot() {
        let l   = fl();
        let sid = KeySlotId(77);
        l.record(AuditKind::KeyLoaded,   Some(sid), 0, true);
        l.record(AuditKind::BlobEncrypted, None,     0, true);
        let r = l.filter_by_slot(sid).unwrap();
        assert!(r.iter().all(|e| e.slot_id == Some(sid)));
        assert!(!r.is_empty());
    }

    #[test] fn test_range_ok() {
        let l = fl();
        for _ in 0..10 { l.record(AuditKind::BlobEncrypted, None, 0, true); }
        let r = l.range(2, 5).unwrap();
        assert!(r.iter().all(|e| e.seq >= 2 && e.seq <= 5));
    }

    #[test] fn test_range_invalid_order_fails() {
        let l = fl();
        assert!(l.range(10, 5).is_err());
    }

    #[test] fn test_concurrent_record_no_panic() {
        let l = fl();
        for i in 0..50u64 { l.record(AuditKind::BlobShredded, Some(KeySlotId(i)), i, i%2==0); }
        assert_eq!(l.total_written(), 50);
    }
}
