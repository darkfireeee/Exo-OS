// SPDX-License-Identifier: MIT
// ExoFS Quota — Namespaces de quota
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::quota_policy::{QuotaPolicy, QuotaLimits, QuotaKind, PolicyFlags};
use super::quota_tracker::QuotaKey;

// ─── Constants ────────────────────────────────────────────────────────────────

pub const NAMESPACE_MAX: usize   = 64;
pub const NS_NAME_LEN:   usize   = 32;
pub const NS_ROOT_ID:    u64     = 0;

// ─── NamespaceId ─────────────────────────────────────────────────────────────

/// Identifiant opaque d'un namespace de quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NamespaceId(pub u64);

impl NamespaceId {
    pub const ROOT: NamespaceId = NamespaceId(NS_ROOT_ID);
    pub fn is_root(self) -> bool { self.0 == NS_ROOT_ID }
    pub fn is_valid(self) -> bool { self.0 != u64::MAX }
    pub const fn invalid() -> Self { NamespaceId(u64::MAX) }
}

// ─── NamespaceFlags ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceFlags(pub u32);

impl NamespaceFlags {
    pub const ACTIVE:    NamespaceFlags = NamespaceFlags(1 << 0);
    pub const READONLY:  NamespaceFlags = NamespaceFlags(1 << 1);
    pub const INHERITED: NamespaceFlags = NamespaceFlags(1 << 2);
    pub const AUDIT:     NamespaceFlags = NamespaceFlags(1 << 3);

    pub const fn default_flags() -> Self { Self(Self::ACTIVE.0 | Self::AUDIT.0) }
    pub fn has(self, f: NamespaceFlags) -> bool { self.0 & f.0 != 0 }
    pub fn set(self, f: NamespaceFlags) -> Self  { NamespaceFlags(self.0 | f.0) }
    pub fn clear(self, f: NamespaceFlags) -> Self { NamespaceFlags(self.0 & !f.0) }
    pub fn is_active(self)    -> bool { self.has(Self::ACTIVE) }
    pub fn is_readonly(self)  -> bool { self.has(Self::READONLY) }
    pub fn is_inherited(self) -> bool { self.has(Self::INHERITED) }
    pub fn audit_enabled(self) -> bool { self.has(Self::AUDIT) }
}

// ─── QuotaNamespaceEntry ──────────────────────────────────────────────────────

/// Entrée d'un namespace de quota.
#[derive(Clone, Copy, Debug)]
pub struct QuotaNamespaceEntry {
    pub id:        NamespaceId,
    pub parent_id: NamespaceId,
    pub policy:    QuotaPolicy,
    pub flags:     NamespaceFlags,
    pub name:      [u8; NS_NAME_LEN],
    /// Nombre d'entités suivies dans ce namespace.
    pub entity_count: u64,
    /// Tick de création.
    pub created_tick: u64,
    pub occupied:  bool,
}

impl QuotaNamespaceEntry {
    pub const fn empty() -> Self {
        Self {
            id:           NamespaceId::invalid(),
            parent_id:    NamespaceId::invalid(),
            policy:       QuotaPolicy::default_policy(QuotaKind::User),
            flags:        NamespaceFlags::default_flags(),
            name:         [0u8; NS_NAME_LEN],
            entity_count: 0,
            created_tick: 0,
            occupied:     false,
        }
    }

    pub fn new(
        id:        NamespaceId,
        parent_id: NamespaceId,
        policy:    QuotaPolicy,
        name:      &str,
        tick:      u64,
    ) -> Self {
        let mut e = Self::empty();
        e.id           = id;
        e.parent_id    = parent_id;
        e.policy       = policy;
        e.flags        = NamespaceFlags::default_flags();
        e.created_tick = tick;
        e.occupied     = true;
        let bytes = name.as_bytes();
        let len = bytes.len().min(NS_NAME_LEN);
        let mut i = 0usize;
        while i < len { e.name[i] = bytes[i]; i = i.wrapping_add(1); }
        e
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NS_NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    pub fn is_active(&self)   -> bool { self.flags.is_active() }
    pub fn is_readonly(&self) -> bool { self.flags.is_readonly() }

    /// Vrai si ce namespace hérite les limites du parent.
    pub fn is_inherited(&self) -> bool { self.flags.is_inherited() }

    /// Retourne la clé de quota pour l'entité `id` dans ce namespace.
    pub fn key_for(&self, entity_id: u64) -> QuotaKey {
        QuotaKey::new(self.policy.kind, entity_id)
    }

    /// Score de charge : entity_count / hard_bytes (ARITH-02).
    pub fn load_score(&self) -> u64 {
        if self.policy.limits.hard_bytes == 0 || self.policy.limits.hard_bytes == u64::MAX {
            return 0;
        }
        self.entity_count.saturating_mul(1000)
            .checked_div(self.policy.limits.hard_bytes).unwrap_or(0)
    }
}

// ─── QuotaNamespace ───────────────────────────────────────────────────────────

/// Registre plat de namespaces de quota (max NAMESPACE_MAX).
pub struct QuotaNamespace {
    entries: UnsafeCell<[QuotaNamespaceEntry; NAMESPACE_MAX]>,
    count:   AtomicU64,
    lock:    AtomicU64,
    next_id: AtomicU64,
}

unsafe impl Sync for QuotaNamespace {}
unsafe impl Send for QuotaNamespace {}

impl QuotaNamespace {
    pub const fn new_const() -> Self {
        Self {
            entries: UnsafeCell::new([QuotaNamespaceEntry::empty(); NAMESPACE_MAX]),
            count:   AtomicU64::new(0),
            lock:    AtomicU64::new(0),
            next_id: AtomicU64::new(1), // 0 = ROOT (pré-créé)
        }
    }

    fn acquire(&self) {
        while self.lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn release(&self) { self.lock.store(0, Ordering::Release); }

    fn find_idx(&self, id: NamespaceId) -> Option<usize> {
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < NAMESPACE_MAX {
            if entries[i].occupied && entries[i].id == id { return Some(i); }
            i = i.wrapping_add(1);
        }
        None
    }

    fn find_free_slot(&self) -> Option<usize> {
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < NAMESPACE_MAX {
            if !entries[i].occupied { return Some(i); }
            i = i.wrapping_add(1);
        }
        None
    }

    fn find_by_name(&self, name: &str) -> Option<usize> {
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < NAMESPACE_MAX {
            if entries[i].occupied && entries[i].name_str() == name { return Some(i); }
            i = i.wrapping_add(1);
        }
        None
    }

    /// Alloue un nouveau NamespaceId.
    fn alloc_id(&self) -> NamespaceId {
        NamespaceId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    // ── API publique ──────────────────────────────────────────────────────────

    /// Ajoute un namespace avec la politique donnée.
    pub fn add(
        &self,
        parent_id: NamespaceId,
        policy:    QuotaPolicy,
        name:      &str,
        tick:      u64,
    ) -> ExofsResult<NamespaceId> {
        policy.validate()?;
        if name.is_empty() || name.len() > NS_NAME_LEN {
            return Err(ExofsError::InvalidArgument);
        }
        self.acquire();
        let result = self._add_locked(parent_id, policy, name, tick);
        self.release();
        result
    }

    fn _add_locked(
        &self,
        parent_id: NamespaceId,
        policy:    QuotaPolicy,
        name:      &str,
        tick:      u64,
    ) -> ExofsResult<NamespaceId> {
        // Vérifier que le parent existe (sauf ROOT)
        if !parent_id.is_root() && self.find_idx(parent_id).is_none() {
            return Err(ExofsError::ObjectNotFound);
        }
        // Vérifier le nom unique
        if self.find_by_name(name).is_some() {
            return Err(ExofsError::ObjectAlreadyExists);
        }
        let slot = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
        let id = self.alloc_id();
        let entries = unsafe { &mut *self.entries.get() };
        entries[slot] = QuotaNamespaceEntry::new(id, parent_id, policy, name, tick);
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    /// Supprime un namespace (doit être vide).
    pub fn remove(&self, id: NamespaceId) -> ExofsResult<()> {
        if id.is_root() { return Err(ExofsError::PermissionDenied); }
        self.acquire();
        let result = self._remove_locked(id);
        self.release();
        result
    }

    fn _remove_locked(&self, id: NamespaceId) -> ExofsResult<()> {
        let entries = unsafe { &mut *self.entries.get() };
        let idx = self.find_idx(id).ok_or(ExofsError::ObjectNotFound)?;
        if entries[idx].entity_count > 0 {
            return Err(ExofsError::DirectoryNotEmpty);
        }
        entries[idx] = QuotaNamespaceEntry::empty();
        self.count.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    /// Lit une entrée par id.
    pub fn get(&self, id: NamespaceId) -> Option<QuotaNamespaceEntry> {
        self.acquire();
        let entries = unsafe { &*self.entries.get() };
        let r = self.find_idx(id).map(|i| entries[i]);
        self.release();
        r
    }

    /// Recherche par nom.
    pub fn lookup_by_name(&self, name: &str) -> Option<QuotaNamespaceEntry> {
        self.acquire();
        let entries = unsafe { &*self.entries.get() };
        let r = self.find_by_name(name).map(|i| entries[i]);
        self.release();
        r
    }

    /// Met à jour la politique d'un namespace.
    pub fn set_policy(&self, id: NamespaceId, policy: QuotaPolicy) -> ExofsResult<()> {
        policy.validate()?;
        self.acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = self.find_idx(id) {
            if entries[idx].is_readonly() { Err(ExofsError::PermissionDenied) }
            else { entries[idx].policy = policy; Ok(()) }
        } else { Err(ExofsError::ObjectNotFound) };
        self.release();
        result
    }

    /// Met à jour les flags d'un namespace.
    pub fn set_flags(&self, id: NamespaceId, flags: NamespaceFlags) -> ExofsResult<()> {
        if id.is_root() { return Err(ExofsError::PermissionDenied); }
        self.acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = self.find_idx(id) {
            entries[idx].flags = flags; Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.release();
        result
    }

    /// Incrémente le compteur d'entités d'un namespace.
    pub fn inc_entity(&self, id: NamespaceId) -> ExofsResult<()> {
        self.acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = self.find_idx(id) {
            entries[idx].entity_count = entries[idx].entity_count.saturating_add(1);
            Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.release();
        result
    }

    /// Décrémente le compteur d'entités d'un namespace.
    pub fn dec_entity(&self, id: NamespaceId) -> ExofsResult<()> {
        self.acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = self.find_idx(id) {
            entries[idx].entity_count = entries[idx].entity_count.saturating_sub(1);
            Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.release();
        result
    }

    /// Nombre de namespaces actifs.
    pub fn count(&self) -> usize { self.count.load(Ordering::Relaxed) as usize }

    /// Snapshot de tous les namespaces (OOM-02, RECUR-01).
    pub fn list_all(&self) -> ExofsResult<Vec<QuotaNamespaceEntry>> {
        let n = self.count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < NAMESPACE_MAX {
            if entries[i].occupied {
                v.try_reserve(1).map_err(|_| { self.release(); ExofsError::NoMemory })?;
                v.push(entries[i]);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Liste les enfants d'un namespace parent (RECUR-01 : while).
    pub fn children_of(&self, parent_id: NamespaceId) -> ExofsResult<Vec<NamespaceId>> {
        let mut v = Vec::new();
        v.try_reserve(8).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        let entries = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < NAMESPACE_MAX {
            if entries[i].occupied && entries[i].parent_id == parent_id {
                v.try_reserve(1).map_err(|_| { self.release(); ExofsError::NoMemory })?;
                v.push(entries[i].id);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Retourne les limites effectives (héritées si INHERITED flag).
    pub fn effective_limits(&self, id: NamespaceId) -> Option<QuotaLimits> {
        self.acquire();
        let entries = unsafe { &*self.entries.get() };
        let result = if let Some(idx) = self.find_idx(id) {
            let e = &entries[idx];
            if e.is_inherited() {
                // Chercher les limites du parent
                if let Some(pidx) = self.find_idx(e.parent_id) {
                    Some(entries[pidx].policy.limits)
                } else {
                    Some(e.policy.limits)
                }
            } else {
                Some(e.policy.limits)
            }
        } else { None };
        self.release();
        result
    }

    /// Initialise le namespace root (id=0).
    pub fn init_root(&self, policy: QuotaPolicy, tick: u64) -> ExofsResult<()> {
        policy.validate()?;
        self.acquire();
        let entries = unsafe { &mut *self.entries.get() };
        // Trouver un slot libre pour le root
        let slot = {
            let mut s = None;
            let mut i = 0usize;
            while i < NAMESPACE_MAX {
                if !entries[i].occupied { s = Some(i); break; }
                i = i.wrapping_add(1);
            }
            s.ok_or(ExofsError::NoSpace)
        };
        let result = match slot {
            Ok(s) => {
                entries[s] = QuotaNamespaceEntry::new(
                    NamespaceId::ROOT,
                    NamespaceId::ROOT,
                    policy,
                    "root",
                    tick,
                );
                self.count.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => Err(e),
        };
        self.release();
        result
    }
}

/// Singleton global du registre de namespaces.
pub static QUOTA_NAMESPACE: QuotaNamespace = QuotaNamespace::new_const();

// ─── NamespaceStats ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct NamespaceStats {
    pub id:            NamespaceId,
    pub entity_count:  u64,
    pub usage_score:   u64,
    pub is_active:     bool,
    pub is_readonly:   bool,
}

impl NamespaceStats {
    pub fn from_entry(e: &QuotaNamespaceEntry) -> Self {
        Self {
            id:           e.id,
            entity_count: e.entity_count,
            usage_score:  e.load_score(),
            is_active:    e.is_active(),
            is_readonly:  e.is_readonly(),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::quota::quota_policy::{PolicyPresets, QuotaKind};

    fn default_policy() -> QuotaPolicy { PolicyPresets::standard_user() }

    #[test]
    fn test_namespace_id_root() {
        assert!(NamespaceId::ROOT.is_root());
        assert!(!NamespaceId(1).is_root());
        assert!(!NamespaceId::invalid().is_valid());
    }

    #[test]
    fn test_namespace_flags() {
        let f = NamespaceFlags::default_flags();
        assert!(f.is_active());
        assert!(f.audit_enabled());
        assert!(!f.is_readonly());
        let f2 = f.set(NamespaceFlags::READONLY);
        assert!(f2.is_readonly());
    }

    #[test]
    fn test_entry_name_str() {
        let e = QuotaNamespaceEntry::new(
            NamespaceId(1), NamespaceId::ROOT, default_policy(), "testns", 0
        );
        assert_eq!(e.name_str(), "testns");
    }

    #[test]
    fn test_registry_add_and_get() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        let id = ns.add(NamespaceId::ROOT, default_policy(), "myns", 10).expect("add");
        let got = ns.get(id).expect("found");
        assert_eq!(got.name_str(), "myns");
        assert_eq!(got.parent_id, NamespaceId::ROOT);
    }

    #[test]
    fn test_registry_duplicate_name() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        ns.add(NamespaceId::ROOT, default_policy(), "dup", 0).expect("first");
        assert!(ns.add(NamespaceId::ROOT, default_policy(), "dup", 0).is_err());
    }

    #[test]
    fn test_registry_remove() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        let id = ns.add(NamespaceId::ROOT, default_policy(), "torm", 0).expect("ok");
        assert!(ns.get(id).is_some());
        ns.remove(id).expect("remove");
        assert!(ns.get(id).is_none());
    }

    #[test]
    fn test_registry_remove_root_denied() {
        let ns = QuotaNamespace::new_const();
        assert!(ns.remove(NamespaceId::ROOT).is_err());
    }

    #[test]
    fn test_registry_remove_nonempty() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        let id = ns.add(NamespaceId::ROOT, default_policy(), "nonempty", 0).expect("ok");
        ns.inc_entity(id).expect("inc");
        assert!(ns.remove(id).is_err());
    }

    #[test]
    fn test_registry_set_policy() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        let id = ns.add(NamespaceId::ROOT, default_policy(), "p", 0).expect("ok");
        let new_p = PolicyPresets::sandbox();
        ns.set_policy(id, new_p).expect("set");
        let got = ns.get(id).expect("found");
        assert_eq!(got.policy.limits.hard_bytes, new_p.limits.hard_bytes);
    }

    #[test]
    fn test_registry_entity_count() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        let id = ns.add(NamespaceId::ROOT, default_policy(), "cnt", 0).expect("ok");
        ns.inc_entity(id).expect("inc");
        ns.inc_entity(id).expect("inc");
        let e = ns.get(id).expect("found");
        assert_eq!(e.entity_count, 2);
        ns.dec_entity(id).expect("dec");
        let e2 = ns.get(id).expect("found");
        assert_eq!(e2.entity_count, 1);
    }

    #[test]
    fn test_registry_children_of() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        let c1 = ns.add(NamespaceId::ROOT, default_policy(), "c1", 0).expect("c1");
        let c2 = ns.add(NamespaceId::ROOT, default_policy(), "c2", 0).expect("c2");
        let _ = ns.add(c1, default_policy(), "gc1", 0).expect("gc1");
        let children = ns.children_of(NamespaceId::ROOT).expect("ok");
        assert_eq!(children.len(), 2);
        assert!(children.contains(&c1));
        assert!(children.contains(&c2));
    }

    #[test]
    fn test_registry_list_all() {
        let ns = QuotaNamespace::new_const();
        ns.init_root(default_policy(), 0).expect("root");
        ns.add(NamespaceId::ROOT, default_policy(), "a", 0).expect("a");
        ns.add(NamespaceId::ROOT, default_policy(), "b", 0).expect("b");
        let all = ns.list_all().expect("ok");
        assert_eq!(all.len(), 3); // root + a + b
    }

    #[test]
    fn test_namespace_stats() {
        let e = QuotaNamespaceEntry::new(NamespaceId(1), NamespaceId::ROOT, default_policy(), "s", 0);
        let stats = NamespaceStats::from_entry(&e);
        assert_eq!(stats.id, NamespaceId(1));
        assert!(stats.is_active);
        assert!(!stats.is_readonly);
    }
}
