//! snapshot_protect.rs — Protection et rétention WORM des snapshots ExoFS
//!
//! Gère le flag PROTECTED ainsi qu'une politique de rétention immuable
//! (mode WORM : Write-Once-Read-Many). Un snapshot sous WORM ne peut
//! être déprotégé qu'après expiration de la durée de rétention.
//!
//! Règles spec :
//!   ARITH-02 : checked_add pour timestamps
//!   OOM-02   : try_reserve avant chaque push

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::snapshot::flags;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────

pub static SNAPSHOT_PROTECT: SnapshotProtect = SnapshotProtect::new_const();

// ─────────────────────────────────────────────────────────────
// Politique WORM
// ─────────────────────────────────────────────────────────────

/// Politique WORM pour un snapshot
#[derive(Debug, Clone, Copy)]
pub struct WormPolicy {
    /// Timestamp d'activation (ticks)
    pub activated_at: u64,
    /// Durée de rétention (ticks) — 0 = permanente
    pub retain_ticks: u64,
}

impl WormPolicy {
    /// Retourne true si la politique a expiré
    pub fn is_expired(&self, now: u64) -> bool {
        if self.retain_ticks == 0 {
            return false;
        }
        now.saturating_sub(self.activated_at) >= self.retain_ticks
    }

    /// Timestamp d'expiration (0 si permanente)
    pub fn expires_at(&self) -> u64 {
        if self.retain_ticks == 0 {
            return 0;
        }
        self.activated_at.saturating_add(self.retain_ticks)
    }
}

// ─────────────────────────────────────────────────────────────
// Entrée de protection
// ─────────────────────────────────────────────────────────────

/// Entrée de protection d'un snapshot
#[derive(Debug, Clone)]
pub struct ProtectEntry {
    pub snap_id: SnapshotId,
    pub worm: Option<WormPolicy>,
    pub protected_at: u64,
    /// Note libre (utf-8, max 64 octets)
    pub note: [u8; 64],
}

impl ProtectEntry {
    fn new(snap_id: SnapshotId, worm: Option<WormPolicy>, now: u64) -> Self {
        Self {
            snap_id,
            worm,
            protected_at: now,
            note: [0u8; 64],
        }
    }

    pub fn is_worm(&self) -> bool {
        self.worm.is_some()
    }

    pub fn can_unprotect(&self, now: u64) -> bool {
        match &self.worm {
            None => true,
            Some(worm) => worm.is_expired(now),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotProtect
// ─────────────────────────────────────────────────────────────

pub struct SnapshotProtect {
    entries: SpinLock<alloc::collections::BTreeMap<u64, ProtectEntry>>,
    n_worm: AtomicUsize,
    n_protected: AtomicUsize,
}

impl SnapshotProtect {
    pub const fn new_const() -> Self {
        Self {
            entries: SpinLock::new(alloc::collections::BTreeMap::new()),
            n_worm: AtomicUsize::new(0),
            n_protected: AtomicUsize::new(0),
        }
    }

    // ── Protéger ────────────────────────────────────────────────────

    /// Protège un snapshot (flag PROTECTED)
    pub fn protect(&self, snap_id: SnapshotId, now: u64) -> ExofsResult<()> {
        self.protect_inner(snap_id, None, now)
    }

    /// Protège un snapshot avec rétention WORM (`retain_ticks` ticks)
    pub fn protect_worm(
        &self,
        snap_id: SnapshotId,
        retain_ticks: u64,
        now: u64,
    ) -> ExofsResult<()> {
        let worm = WormPolicy {
            activated_at: now,
            retain_ticks,
        };
        self.protect_inner(snap_id, Some(worm), now)
    }

    fn protect_inner(
        &self,
        snap_id: SnapshotId,
        worm: Option<WormPolicy>,
        now: u64,
    ) -> ExofsResult<()> {
        // Vérifier que le snapshot existe
        let _ = SNAPSHOT_LIST.get_ref(snap_id)?;

        // Poser le flag dans le registre
        SNAPSHOT_LIST.set_flags(snap_id, flags::PROTECTED)?;

        let entry = ProtectEntry::new(snap_id, worm, now);
        let is_worm = entry.is_worm();

        let mut guard = self.entries.lock();
        guard.insert(snap_id.0, entry);
        drop(guard);

        self.n_protected.fetch_add(1, Ordering::AcqRel);
        if is_worm {
            self.n_worm.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }

    // ── Déprotéger ───────────────────────────────────────────────────

    /// Retire la protection d'un snapshot
    pub fn unprotect(&self, snap_id: SnapshotId, now: u64) -> ExofsResult<()> {
        let mut guard = self.entries.lock();
        let entry = guard.get(&snap_id.0).ok_or(ExofsError::NotFound)?;

        if !entry.can_unprotect(now) {
            return Err(ExofsError::InvalidState); // WORM non expiré
        }

        let was_worm = entry.is_worm();
        guard.remove(&snap_id.0);
        drop(guard);

        SNAPSHOT_LIST.clear_flags(snap_id, flags::PROTECTED)?;
        self.n_protected.fetch_sub(1, Ordering::AcqRel);
        if was_worm {
            self.n_worm.fetch_sub(1, Ordering::AcqRel);
        }
        Ok(())
    }

    // ── Requêtes ────────────────────────────────────────────────────

    /// Retourne true si le snapshot est protégé
    pub fn is_protected(&self, snap_id: SnapshotId) -> bool {
        let guard = self.entries.lock();
        guard.contains_key(&snap_id.0)
    }

    /// Retourne l'entrée de protection (si elle existe)
    pub fn get_entry(&self, snap_id: SnapshotId) -> Option<ProtectEntry> {
        let guard = self.entries.lock();
        guard.get(&snap_id.0).cloned()
    }

    /// Retourne true si le snapshot est sous WORM et non expiré
    pub fn is_worm_active(&self, snap_id: SnapshotId, now: u64) -> bool {
        let guard = self.entries.lock();
        guard
            .get(&snap_id.0)
            .and_then(|e| e.worm.as_ref())
            .map_or(false, |w| !w.is_expired(now))
    }

    // ── Statistiques ────────────────────────────────────────────────

    pub fn n_protected(&self) -> usize {
        self.n_protected.load(Ordering::Acquire)
    }
    pub fn n_worm(&self) -> usize {
        self.n_worm.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> ProtectStats {
        let guard = self.entries.lock();
        let mut n_expiring_soon: usize = 0;
        let mut n_permanent: usize = 0;
        for e in guard.values() {
            match &e.worm {
                None => n_permanent += 1,
                Some(w) => {
                    if w.retain_ticks > 0 {
                        n_expiring_soon += 1;
                    }
                }
            }
        }
        ProtectStats {
            n_protected: guard.len(),
            n_worm: self.n_worm.load(Ordering::Acquire),
            n_permanent,
            n_expiring_soon,
        }
    }

    // ── Liste des entrées expirées ────────────────────────────────────

    /// Retourne les snapshots WORM dont la rétention a expiré
    pub fn expired_worm(&self, now: u64) -> ExofsResult<Vec<SnapshotId>> {
        let guard = self.entries.lock();
        let mut out: Vec<SnapshotId> = Vec::new();
        for e in guard.values() {
            if let Some(worm) = &e.worm {
                if worm.is_expired(now) {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(e.snap_id);
                }
            }
        }
        Ok(out)
    }

    /// Retourne tous les snapshots protégés (ids)
    pub fn all_protected(&self) -> ExofsResult<Vec<SnapshotId>> {
        let guard = self.entries.lock();
        let mut out: Vec<SnapshotId> = Vec::new();
        out.try_reserve(guard.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for &k in guard.keys() {
            out.push(SnapshotId(k));
        }
        Ok(out)
    }

    // ── Ajout de note ────────────────────────────────────────────────

    pub fn set_note(&self, snap_id: SnapshotId, note: &[u8]) -> ExofsResult<()> {
        let mut guard = self.entries.lock();
        let entry = guard.get_mut(&snap_id.0).ok_or(ExofsError::NotFound)?;
        let len = note.len().min(64);
        entry.note[..len].copy_from_slice(&note[..len]);
        if len < 64 {
            entry.note[len..].fill(0);
        }
        Ok(())
    }

    // ── Nettoyage ────────────────────────────────────────────────────

    pub fn clear(&self) {
        let mut guard = self.entries.lock();
        guard.clear();
        self.n_protected.store(0, Ordering::Release);
        self.n_worm.store(0, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ProtectStats {
    pub n_protected: usize,
    pub n_worm: usize,
    pub n_permanent: usize,
    pub n_expiring_soon: usize,
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::snapshot::{make_snapshot_name, Snapshot};
    use super::super::reset_for_test;
    use super::super::snapshot_list::{SnapshotList, SNAPSHOT_LIST};
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};

    fn push_snap(_list: &SnapshotList, id: u64) {
        SNAPSHOT_LIST.register(Snapshot {
            id: SnapshotId(id),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"p-test"),
        })
        .unwrap();
    }

    #[test]
    fn protect_and_is_protected() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1);
        let p = SnapshotProtect::new_const();
        p.protect(SnapshotId(1), 0).unwrap();
        assert!(p.is_protected(SnapshotId(1)));
        // Flag dans la liste
        let snap = SNAPSHOT_LIST.get(SnapshotId(1)).unwrap();
        assert!(snap.is_protected());
    }

    #[test]
    fn unprotect_without_worm() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 2);
        let p = SnapshotProtect::new_const();
        p.protect(SnapshotId(2), 0).unwrap();
        p.unprotect(SnapshotId(2), 999).unwrap();
        assert!(!p.is_protected(SnapshotId(2)));
    }

    #[test]
    fn worm_blocks_early_unprotect() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 3);
        let p = SnapshotProtect::new_const();
        p.protect_worm(SnapshotId(3), 1000, 0).unwrap();
        // Trop tôt
        let err = p.unprotect(SnapshotId(3), 500);
        assert!(matches!(err, Err(ExofsError::InvalidState)));
    }

    #[test]
    fn worm_allows_unprotect_after_expiry() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 4);
        let p = SnapshotProtect::new_const();
        p.protect_worm(SnapshotId(4), 500, 0).unwrap();
        p.unprotect(SnapshotId(4), 600).unwrap();
        assert!(!p.is_protected(SnapshotId(4)));
    }

    #[test]
    fn expired_worm_list() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 10);
        push_snap(&list, 11);
        let p = SnapshotProtect::new_const();
        p.protect_worm(SnapshotId(10), 500, 0).unwrap();
        p.protect_worm(SnapshotId(11), 2000, 0).unwrap();
        let expired = p.expired_worm(1000).unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, 10);
    }

    #[test]
    fn stats_count_correct() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 20);
        push_snap(&list, 21);
        let p = SnapshotProtect::new_const();
        p.protect(SnapshotId(20), 0).unwrap();
        p.protect_worm(SnapshotId(21), 999, 0).unwrap();
        let s = p.stats();
        assert_eq!(s.n_protected, 2);
        assert_eq!(s.n_worm, 1);
    }
}
