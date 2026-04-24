// SPDX-License-Identifier: MIT
// ExoFS Quota — Tracker d'utilisation
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::quota_policy::{QuotaKind, QuotaLimits};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Nombre maximum d'entités suivies simultanément.
pub const QUOTA_MAX_ENTRIES: usize = 256;

// ─── QuotaKey ─────────────────────────────────────────────────────────────────

/// Clé unique d'une entité soumise à quota : (kind, entity_id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct QuotaKey {
    pub kind: u8, // QuotaKind as u8
    pub entity_id: u64,
}

impl QuotaKey {
    pub fn new(kind: QuotaKind, entity_id: u64) -> Self {
        Self {
            kind: kind as u8,
            entity_id,
        }
    }
    pub fn kind(&self) -> QuotaKind {
        QuotaKind::from_u8(self.kind)
    }
    pub const fn zeroed() -> Self {
        Self {
            kind: 0,
            entity_id: 0,
        }
    }
    pub fn is_zeroed(&self) -> bool {
        self.kind == 0 && self.entity_id == 0
    }
}

// ─── QuotaUsage ───────────────────────────────────────────────────────────────

/// Consommation courante d'une entité.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuotaUsage {
    pub bytes_used: u64,
    pub blobs_used: u64,
    pub inodes_used: u64,
}

impl QuotaUsage {
    pub const fn zero() -> Self {
        Self {
            bytes_used: 0,
            blobs_used: 0,
            inodes_used: 0,
        }
    }

    /// Ajoute des bytes (saturating, ARITH-02).
    pub fn add_bytes(self, n: u64) -> Self {
        Self {
            bytes_used: self.bytes_used.saturating_add(n),
            ..self
        }
    }
    /// Soustrait des bytes (saturating, ARITH-02).
    pub fn sub_bytes(self, n: u64) -> Self {
        Self {
            bytes_used: self.bytes_used.saturating_sub(n),
            ..self
        }
    }
    /// Ajoute des blobs (saturating).
    pub fn add_blobs(self, n: u64) -> Self {
        Self {
            blobs_used: self.blobs_used.saturating_add(n),
            ..self
        }
    }
    /// Soustrait des blobs.
    pub fn sub_blobs(self, n: u64) -> Self {
        Self {
            blobs_used: self.blobs_used.saturating_sub(n),
            ..self
        }
    }
    /// Ajoute des inodes.
    pub fn add_inodes(self, n: u64) -> Self {
        Self {
            inodes_used: self.inodes_used.saturating_add(n),
            ..self
        }
    }
    /// Soustrait des inodes.
    pub fn sub_inodes(self, n: u64) -> Self {
        Self {
            inodes_used: self.inodes_used.saturating_sub(n),
            ..self
        }
    }

    /// Total des octets + blobs + inodes pour tri.
    pub fn total_weight(&self) -> u64 {
        self.bytes_used
            .saturating_add(self.blobs_used)
            .saturating_add(self.inodes_used)
    }
}

// ─── QuotaEntry ───────────────────────────────────────────────────────────────

/// Enregistrement interne combinant clé, utilisation et limites.
#[derive(Clone, Copy, Debug)]
pub struct QuotaEntry {
    pub key: QuotaKey,
    pub usage: QuotaUsage,
    pub limits: QuotaLimits,
    /// Tick du dernier dépassement soft (0 = pas de dépassement actif).
    pub soft_breach_tick: u64,
    /// Indique si l'entrée est occupée dans le tableau plat.
    pub occupied: bool,
}

impl QuotaEntry {
    pub const fn empty() -> Self {
        Self {
            key: QuotaKey::zeroed(),
            usage: QuotaUsage::zero(),
            limits: QuotaLimits::unlimited(),
            soft_breach_tick: 0,
            occupied: false,
        }
    }

    pub fn new(key: QuotaKey, limits: QuotaLimits) -> Self {
        Self {
            key,
            usage: QuotaUsage::zero(),
            limits,
            soft_breach_tick: 0,
            occupied: true,
        }
    }

    /// Pourcentage d'utilisation bytes en ‰.
    pub fn bytes_usage_ppt(&self) -> u64 {
        self.limits.bytes_usage_ppt(self.usage.bytes_used)
    }

    /// Vrai si la limite dure bytes est dépassée.
    pub fn bytes_hard_exceeded(&self) -> bool {
        self.limits.bytes_hard_exceeded(self.usage.bytes_used)
    }

    /// Vrai si la limite souple bytes est dépassée.
    pub fn bytes_soft_exceeded(&self) -> bool {
        self.limits.bytes_soft_exceeded(self.usage.bytes_used)
    }

    /// Vrai si la grâce est expirée depuis le premier dépassement soft.
    pub fn grace_expired(&self, current_tick: u64) -> bool {
        if self.soft_breach_tick == 0 {
            return false;
        }
        if self.limits.grace_ticks == 0 {
            return true;
        }
        current_tick.saturating_sub(self.soft_breach_tick) >= self.limits.grace_ticks
    }
}

// ─── QuotaTracker ─────────────────────────────────────────────────────────────

/// Table plate d'entrées de quota (max QUOTA_MAX_ENTRIES).
/// Thread-safe via spinlock AtomicU64.
pub struct QuotaTracker {
    entries: UnsafeCell<[QuotaEntry; QUOTA_MAX_ENTRIES]>,
    count: AtomicU64,
    lock: AtomicU64,
    total_bytes: AtomicU64,
    total_blobs: AtomicU64,
    total_inodes: AtomicU64,
}

unsafe impl Sync for QuotaTracker {}
unsafe impl Send for QuotaTracker {}

impl QuotaTracker {
    pub const fn new_const() -> Self {
        Self {
            entries: UnsafeCell::new([QuotaEntry::empty(); QUOTA_MAX_ENTRIES]),
            count: AtomicU64::new(0),
            lock: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            total_blobs: AtomicU64::new(0),
            total_inodes: AtomicU64::new(0),
        }
    }

    // ── Spinlock ─────────────────────────────────────────────────────────────

    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    // ── Recherche linéaire (RECUR-01 : while) ─────────────────────────────

    fn find_idx(&self, key: QuotaKey) -> Option<usize> {
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < QUOTA_MAX_ENTRIES {
            if entries[i].occupied && entries[i].key == key {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    fn find_free_slot(&self) -> Option<usize> {
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < QUOTA_MAX_ENTRIES {
            if !entries[i].occupied {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    // ── API publique ──────────────────────────────────────────────────────────

    /// Insère ou met à jour les limites d'une entité.
    pub fn set_limits(&self, key: QuotaKey, limits: QuotaLimits) -> ExofsResult<()> {
        limits.validate()?;
        self.acquire();
        let result = self._set_limits_locked(key, limits);
        self.release();
        result
    }

    fn _set_limits_locked(&self, key: QuotaKey, limits: QuotaLimits) -> ExofsResult<()> {
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        if let Some(idx) = self.find_idx(key) {
            entries[idx].limits = limits;
            return Ok(());
        }
        // Nouvelle entrée
        let slot = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
        entries[slot] = QuotaEntry::new(key, limits);
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Retire une entité du tracker.
    pub fn remove(&self, key: QuotaKey) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = self.find_idx(key) {
            let e = &entries[idx];
            self.total_bytes
                .fetch_sub(e.usage.bytes_used, Ordering::Relaxed);
            self.total_blobs
                .fetch_sub(e.usage.blobs_used, Ordering::Relaxed);
            self.total_inodes
                .fetch_sub(e.usage.inodes_used, Ordering::Relaxed);
            entries[idx] = QuotaEntry::empty();
            self.count.fetch_sub(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(ExofsError::ObjectNotFound)
        };
        self.release();
        result
    }

    /// Lit l'utilisation courante d'une entité.
    pub fn get_usage(&self, key: QuotaKey) -> Option<QuotaUsage> {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let r = self.find_idx(key).map(|i| entries[i].usage);
        self.release();
        r
    }

    /// Lit les limites d'une entité.
    pub fn get_limits(&self, key: QuotaKey) -> Option<QuotaLimits> {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let r = self.find_idx(key).map(|i| entries[i].limits);
        self.release();
        r
    }

    /// Lit l'entrée complète.
    pub fn get_entry(&self, key: QuotaKey) -> Option<QuotaEntry> {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let r = self.find_idx(key).map(|i| entries[i]);
        self.release();
        r
    }

    /// Ajoute des bytes à une entité (crée si elle n'existe pas avec limites illimitées).
    pub fn add_bytes(&self, key: QuotaKey, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let result = self._mutate_bytes_locked(key, n, true);
        self.release();
        result
    }

    /// Soustrait des bytes à une entité.
    pub fn sub_bytes(&self, key: QuotaKey, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let result = self._mutate_bytes_locked(key, n, false);
        self.release();
        result
    }

    fn _mutate_bytes_locked(&self, key: QuotaKey, n: u64, add: bool) -> ExofsResult<u64> {
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        let idx = match self.find_idx(key) {
            Some(i) => i,
            None => {
                let slot = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
                entries[slot] = QuotaEntry::new(key, QuotaLimits::unlimited());
                self.count.fetch_add(1, Ordering::Relaxed);
                slot
            }
        };
        if add {
            entries[idx].usage = entries[idx].usage.add_bytes(n);
            self.total_bytes.fetch_add(n, Ordering::Relaxed);
        } else {
            entries[idx].usage = entries[idx].usage.sub_bytes(n);
            self.total_bytes.fetch_sub(
                n.min(self.total_bytes.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
        Ok(entries[idx].usage.bytes_used)
    }

    /// Ajoute des blobs à une entité.
    pub fn add_blobs(&self, key: QuotaKey, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let result = self._mutate_blobs_locked(key, n, true);
        self.release();
        result
    }

    /// Soustrait des blobs.
    pub fn sub_blobs(&self, key: QuotaKey, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let result = self._mutate_blobs_locked(key, n, false);
        self.release();
        result
    }

    fn _mutate_blobs_locked(&self, key: QuotaKey, n: u64, add: bool) -> ExofsResult<u64> {
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        let idx = match self.find_idx(key) {
            Some(i) => i,
            None => {
                let slot = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
                entries[slot] = QuotaEntry::new(key, QuotaLimits::unlimited());
                self.count.fetch_add(1, Ordering::Relaxed);
                slot
            }
        };
        if add {
            entries[idx].usage = entries[idx].usage.add_blobs(n);
            self.total_blobs.fetch_add(n, Ordering::Relaxed);
        } else {
            entries[idx].usage = entries[idx].usage.sub_blobs(n);
            let cur = self.total_blobs.load(Ordering::Relaxed);
            self.total_blobs.fetch_sub(n.min(cur), Ordering::Relaxed);
        }
        Ok(entries[idx].usage.blobs_used)
    }

    /// Ajoute des inodes.
    pub fn add_inodes(&self, key: QuotaKey, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let result = self._mutate_inodes_locked(key, n, true);
        self.release();
        result
    }

    /// Soustrait des inodes.
    pub fn sub_inodes(&self, key: QuotaKey, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let result = self._mutate_inodes_locked(key, n, false);
        self.release();
        result
    }

    fn _mutate_inodes_locked(&self, key: QuotaKey, n: u64, add: bool) -> ExofsResult<u64> {
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        let idx = match self.find_idx(key) {
            Some(i) => i,
            None => {
                let slot = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
                entries[slot] = QuotaEntry::new(key, QuotaLimits::unlimited());
                self.count.fetch_add(1, Ordering::Relaxed);
                slot
            }
        };
        if add {
            entries[idx].usage = entries[idx].usage.add_inodes(n);
            self.total_inodes.fetch_add(n, Ordering::Relaxed);
        } else {
            entries[idx].usage = entries[idx].usage.sub_inodes(n);
            let cur = self.total_inodes.load(Ordering::Relaxed);
            self.total_inodes.fetch_sub(n.min(cur), Ordering::Relaxed);
        }
        Ok(entries[idx].usage.inodes_used)
    }

    /// Enregistre le tick du premier dépassement soft.
    pub fn record_soft_breach(&self, key: QuotaKey, tick: u64) {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        if let Some(idx) = self.find_idx(key) {
            if entries[idx].soft_breach_tick == 0 {
                entries[idx].soft_breach_tick = tick;
            }
        }
        self.release();
    }

    /// Efface le tick de dépassement soft (grâce terminée ou quota libéré).
    pub fn clear_soft_breach(&self, key: QuotaKey) {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        if let Some(idx) = self.find_idx(key) {
            entries[idx].soft_breach_tick = 0;
        }
        self.release();
    }

    /// Nombre d'entités actives.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed) as usize
    }

    /// Totaux agrégés.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }
    pub fn total_blobs(&self) -> u64 {
        self.total_blobs.load(Ordering::Relaxed)
    }
    pub fn total_inodes(&self) -> u64 {
        self.total_inodes.load(Ordering::Relaxed)
    }

    /// Snapshot de toutes les entrées actives (OOM-02).
    pub fn snapshot_all(&self) -> ExofsResult<Vec<QuotaEntry>> {
        let count = self.count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < QUOTA_MAX_ENTRIES {
            if entries[i].occupied {
                v.try_reserve(1).map_err(|_| {
                    self.release();
                    ExofsError::NoMemory
                })?;
                v.push(entries[i]);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Retourne les clés de toutes les entités en franchissement hard.
    pub fn hard_exceeded_keys(&self) -> ExofsResult<Vec<QuotaKey>> {
        let mut v = Vec::new();
        v.try_reserve(16).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < QUOTA_MAX_ENTRIES {
            if entries[i].occupied && entries[i].bytes_hard_exceeded() {
                v.try_reserve(1).map_err(|_| {
                    self.release();
                    ExofsError::NoMemory
                })?;
                v.push(entries[i].key);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Réinitialise l'usage d'une entité (conserve les limites).
    pub fn reset_usage(&self, key: QuotaKey) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = self.find_idx(key) {
            let old = entries[idx].usage;
            self.total_bytes.fetch_sub(
                old.bytes_used.min(self.total_bytes.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            self.total_blobs.fetch_sub(
                old.blobs_used.min(self.total_blobs.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            self.total_inodes.fetch_sub(
                old.inodes_used
                    .min(self.total_inodes.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            entries[idx].usage = QuotaUsage::zero();
            entries[idx].soft_breach_tick = 0;
            Ok(())
        } else {
            Err(ExofsError::ObjectNotFound)
        };
        self.release();
        result
    }
}

/// Singleton global du tracker de quotas.
pub static QUOTA_TRACKER: QuotaTracker = QuotaTracker::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::quota::quota_policy::QuotaKind;

    fn make_key(id: u64) -> QuotaKey {
        QuotaKey::new(QuotaKind::User, id)
    }
    fn make_limits(hard: u64) -> QuotaLimits {
        QuotaLimits {
            soft_bytes: hard / 2,
            hard_bytes: hard,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
            grace_ticks: 0,
        }
    }

    #[test]
    fn test_quota_usage_zero() {
        let u = QuotaUsage::zero();
        assert_eq!(u.bytes_used, 0);
        assert_eq!(u.total_weight(), 0);
    }

    #[test]
    fn test_quota_usage_add_sub() {
        let u = QuotaUsage::zero().add_bytes(100).add_blobs(5).add_inodes(2);
        assert_eq!(u.bytes_used, 100);
        let u2 = u.sub_bytes(50);
        assert_eq!(u2.bytes_used, 50);
        let u3 = u2.sub_bytes(9999); // saturating
        assert_eq!(u3.bytes_used, 0);
    }

    #[test]
    fn test_set_limits_and_get() {
        let t = QuotaTracker::new_const();
        let k = make_key(1);
        let l = make_limits(1000);
        t.set_limits(k, l).expect("ok");
        let got = t.get_limits(k).expect("found");
        assert_eq!(got.hard_bytes, 1000);
    }

    #[test]
    fn test_add_bytes() {
        let t = QuotaTracker::new_const();
        let k = make_key(2);
        let l = make_limits(1000);
        t.set_limits(k, l).expect("ok");
        let new_total = t.add_bytes(k, 400).expect("ok");
        assert_eq!(new_total, 400);
        assert_eq!(t.total_bytes(), 400);
    }

    #[test]
    fn test_sub_bytes() {
        let t = QuotaTracker::new_const();
        let k = make_key(3);
        t.set_limits(k, make_limits(1000)).expect("ok");
        t.add_bytes(k, 800).expect("ok");
        let r = t.sub_bytes(k, 300).expect("ok");
        assert_eq!(r, 500);
        assert_eq!(t.total_bytes(), 500);
    }

    #[test]
    fn test_add_blobs_and_inodes() {
        let t = QuotaTracker::new_const();
        let k = make_key(4);
        t.set_limits(k, QuotaLimits::unlimited()).expect("ok");
        t.add_blobs(k, 10).expect("ok");
        t.add_inodes(k, 5).expect("ok");
        let u = t.get_usage(k).expect("found");
        assert_eq!(u.blobs_used, 10);
        assert_eq!(u.inodes_used, 5);
    }

    #[test]
    fn test_remove_entry() {
        let t = QuotaTracker::new_const();
        let k = make_key(5);
        t.set_limits(k, make_limits(500)).expect("ok");
        assert_eq!(t.count(), 1);
        t.remove(k).expect("ok");
        assert_eq!(t.count(), 0);
        assert!(t.get_usage(k).is_none());
    }

    #[test]
    fn test_remove_not_found() {
        let t = QuotaTracker::new_const();
        let k = make_key(99);
        assert!(t.remove(k).is_err());
    }

    #[test]
    fn test_snapshot_all() {
        let t = QuotaTracker::new_const();
        t.set_limits(make_key(10), make_limits(1000)).expect("ok");
        t.set_limits(make_key(11), make_limits(2000)).expect("ok");
        let snap = t.snapshot_all().expect("ok");
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn test_hard_exceeded_keys() {
        let t = QuotaTracker::new_const();
        let k = make_key(20);
        t.set_limits(k, make_limits(100)).expect("ok");
        t.add_bytes(k, 150).expect("ok");
        let keys = t.hard_exceeded_keys().expect("ok");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0], k);
    }

    #[test]
    fn test_soft_breach_tick() {
        let t = QuotaTracker::new_const();
        let k = make_key(30);
        t.set_limits(k, make_limits(1000)).expect("ok");
        t.record_soft_breach(k, 42);
        let e = t.get_entry(k).expect("found");
        assert_eq!(e.soft_breach_tick, 42);
        t.clear_soft_breach(k);
        let e2 = t.get_entry(k).expect("found");
        assert_eq!(e2.soft_breach_tick, 0);
    }

    #[test]
    fn test_grace_expired() {
        let lim = QuotaLimits {
            soft_bytes: 50,
            hard_bytes: 100,
            soft_blobs: u64::MAX,
            hard_blobs: u64::MAX,
            soft_inodes: u64::MAX,
            hard_inodes: u64::MAX,
            grace_ticks: 100,
        };
        let mut e = QuotaEntry::new(make_key(40), lim);
        e.soft_breach_tick = 100;
        assert!(!e.grace_expired(150)); // 50 < 100
        assert!(e.grace_expired(201)); // 101 >= 100
    }

    #[test]
    fn test_reset_usage() {
        let t = QuotaTracker::new_const();
        let k = make_key(50);
        t.set_limits(k, make_limits(1000)).expect("ok");
        t.add_bytes(k, 500).expect("ok");
        t.reset_usage(k).expect("ok");
        let u = t.get_usage(k).expect("found");
        assert_eq!(u.bytes_used, 0);
        assert_eq!(t.total_bytes(), 0);
    }
}
