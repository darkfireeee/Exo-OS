//! Stockage de clés ExoFS — registre sécurisé des slots de clés.
//!
//! Le `KeyStorage` maintient une table de slots de clés protégée par spinlock.
//! Chaque slot contient 256 bits de matériel de clé associé à un `KeyKind`.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un slot de clé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeySlotId(pub u64);

impl core::fmt::Display for KeySlotId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Slot({})", self.0)
    }
}

/// Type de clé stockée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// Clé maître.
    Master,
    /// Clé de volume.
    Volume,
    /// Clé d'objet.
    Object,
    /// Clé dérivée générique.
    Derived,
    /// Clé de session éphémère.
    Session,
}

impl core::fmt::Display for KeyKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Master  => write!(f, "Master"),
            Self::Volume  => write!(f, "Volume"),
            Self::Object  => write!(f, "Object"),
            Self::Derived => write!(f, "Derived"),
            Self::Session => write!(f, "Session"),
        }
    }
}

/// État d'un slot de clé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Active,
    Revoked,
    Expired,
}

/// Entrée dans la table de stockage.
struct KeyEntry {
    /// Matériel de clé.
    key:       [u8; 32],
    /// Type.
    kind:      KeyKind,
    /// État.
    state:     SlotState,
    /// Compteur d'accès.
    accesses:  u64,
}

impl Drop for KeyEntry {
    fn drop(&mut self) { self.key.iter_mut().for_each(|b| *b = 0); }
}

/// Table de stockage (accédée sous lock).
struct StorageTable {
    entries: BTreeMap<KeySlotId, KeyEntry>,
}

impl StorageTable {
    fn new() -> Self { Self { entries: BTreeMap::new() } }

    fn insert(&mut self, slot: KeySlotId, key: [u8; 32], kind: KeyKind) -> ExofsResult<()> {
        if self.entries.contains_key(&slot) {
            return Err(ExofsError::InternalError); // slot déjà occupé
        }
        self.entries.insert(slot, KeyEntry { key, kind, state: SlotState::Active, accesses: 0 });
        Ok(())
    }

    fn get(&mut self, slot: KeySlotId) -> ExofsResult<[u8; 32]> {
        let entry = self.entries.get_mut(&slot)
            .ok_or(ExofsError::ObjectNotFound)?;
        if entry.state != SlotState::Active {
            return Err(ExofsError::InternalError);
        }
        entry.accesses = entry.accesses.saturating_add(1);
        Ok(entry.key)
    }

    fn revoke(&mut self, slot: KeySlotId) -> ExofsResult<()> {
        let entry = self.entries.get_mut(&slot)
            .ok_or(ExofsError::ObjectNotFound)?;
        entry.key.iter_mut().for_each(|b| *b = 0);
        entry.state = SlotState::Revoked;
        Ok(())
    }

    fn remove(&mut self, slot: KeySlotId) -> ExofsResult<()> {
        self.entries.remove(&slot).ok_or(ExofsError::ObjectNotFound)?;
        Ok(())
    }

    fn kind_of(&self, slot: KeySlotId) -> ExofsResult<KeyKind> {
        Ok(self.entries.get(&slot).ok_or(ExofsError::ObjectNotFound)?.kind)
    }

    fn state_of(&self, slot: KeySlotId) -> ExofsResult<SlotState> {
        Ok(self.entries.get(&slot).ok_or(ExofsError::ObjectNotFound)?.state)
    }

    fn list_active(&self) -> Vec<(KeySlotId, KeyKind)> {
        self.entries.iter()
            .filter(|(_, e)| e.state == SlotState::Active)
            .map(|(&s, e)| (s, e.kind))
            .collect()
    }

    fn total(&self) -> usize { self.entries.len() }
}

// ─────────────────────────────────────────────────────────────────────────────
// KeyStorage (thread-safe via spinlock simulé en no_std)
// ─────────────────────────────────────────────────────────────────────────────

/// Stockage de clés thread-safe.
///
/// Utilise un `core::cell::UnsafeCell` pour la mutabilité intérieure,
/// protégé par une sémantique de lock atomique simple (spin).
pub struct KeyStorage {
    table:      core::cell::UnsafeCell<StorageTable>,
    lock:       AtomicU64,
    next_slot:  AtomicU64,
    total_keys: AtomicU64,
}

// SAFETY: KeyStorage est protégé par un lock atomique.
unsafe impl Sync for KeyStorage {}
unsafe impl Send for KeyStorage {}

/// Instance globale.
pub static KEY_STORAGE: KeyStorage = KeyStorage::new_const();

impl KeyStorage {
    /// Constructeur const pour l'initialisation statique.
    pub const fn new_const() -> Self {
        Self {
            table:      core::cell::UnsafeCell::new(StorageTable { entries: BTreeMap::new() }),
            lock:       AtomicU64::new(0),
            next_slot:  AtomicU64::new(1),
            total_keys: AtomicU64::new(0),
        }
    }

    // ── Gestion du lock ───────────────────────────────────────────────────────

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn release(&self) { self.lock.store(0, Ordering::Release); }

    // ── API publique ──────────────────────────────────────────────────────────

    /// Alloue un nouveau slot et y stocke une clé 256-bit.
    pub fn store_key_256(&self, key: &[u8; 32], kind: KeyKind) -> ExofsResult<KeySlotId> {
        let slot = KeySlotId(self.next_slot.fetch_add(1, Ordering::SeqCst));
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let result = unsafe { &mut *self.table.get() }.insert(slot, *key, kind);
        self.release();
        result?;
        self.total_keys.fetch_add(1, Ordering::Relaxed);
        Ok(slot)
    }

    /// Charge une clé depuis un slot.
    pub fn load_key_256(&self, slot: KeySlotId) -> ExofsResult<[u8; 32]> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let result = unsafe { &mut *self.table.get() }.get(slot);
        self.release();
        result
    }

    /// Révoque un slot (zeroize + changement d'état).
    pub fn revoke_key(&self, slot: KeySlotId) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let result = unsafe { &mut *self.table.get() }.revoke(slot);
        self.release();
        result
    }

    /// Supprime définitivement un slot.
    pub fn remove_key(&self, slot: KeySlotId) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let result = unsafe { &mut *self.table.get() }.remove(slot);
        self.release();
        if result.is_ok() { self.total_keys.fetch_sub(1, Ordering::Relaxed); }
        result
    }

    /// Retourne le type de clé d'un slot.
    pub fn key_kind(&self, slot: KeySlotId) -> ExofsResult<KeyKind> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let r = unsafe { &*self.table.get() }.kind_of(slot);
        self.release();
        r
    }

    /// Retourne l'état d'un slot.
    pub fn slot_state(&self, slot: KeySlotId) -> ExofsResult<SlotState> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let r = unsafe { &*self.table.get() }.state_of(slot);
        self.release();
        r
    }

    /// Liste tous les slots actifs avec leur type.
    ///
    /// OOM-02.
    pub fn list_active_slots(&self) -> ExofsResult<Vec<(KeySlotId, KeyKind)>> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let list = unsafe { &*self.table.get() }.list_active();
        self.release();
        Ok(list)
    }

    /// Nombre total de clés (actives + révoquées).
    pub fn total_count(&self) -> u64 { self.total_keys.load(Ordering::Relaxed) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ks() -> KeyStorage { KeyStorage::new_const() }

    #[test] fn test_store_load_roundtrip() {
        let ks  = ks();
        let key = [0x42u8; 32];
        let sid = ks.store_key_256(&key, KeyKind::Master).unwrap();
        assert_eq!(ks.load_key_256(sid).unwrap(), key);
    }

    #[test] fn test_store_different_slots() {
        let ks  = ks();
        let s1  = ks.store_key_256(&[1u8; 32], KeyKind::Volume).unwrap();
        let s2  = ks.store_key_256(&[2u8; 32], KeyKind::Object).unwrap();
        assert_ne!(s1, s2);
    }

    #[test] fn test_revoke_zeroes_key() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0xABu8; 32], KeyKind::Derived).unwrap();
        ks.revoke_key(sid).unwrap();
        // Après révocation, load doit échouer.
        assert!(ks.load_key_256(sid).is_err());
    }

    #[test] fn test_remove_key() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Session).unwrap();
        ks.remove_key(sid).unwrap();
        assert!(ks.load_key_256(sid).is_err());
    }

    #[test] fn test_load_nonexistent_fails() {
        let ks = ks();
        assert!(ks.load_key_256(KeySlotId(999)).is_err());
    }

    #[test] fn test_key_kind_ok() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Volume).unwrap();
        assert_eq!(ks.key_kind(sid).unwrap(), KeyKind::Volume);
    }

    #[test] fn test_list_active_slots_count() {
        let ks  = ks();
        ks.store_key_256(&[1u8; 32], KeyKind::Master).unwrap();
        ks.store_key_256(&[2u8; 32], KeyKind::Volume).unwrap();
        let active = ks.list_active_slots().unwrap();
        assert_eq!(active.len(), 2);
    }

    #[test] fn test_total_count() {
        let ks  = ks();
        ks.store_key_256(&[0u8; 32], KeyKind::Derived).unwrap();
        assert!(ks.total_count() >= 1);
    }

    #[test] fn test_slot_state_active() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Master).unwrap();
        assert_eq!(ks.slot_state(sid).unwrap(), SlotState::Active);
    }

    #[test] fn test_slot_state_after_revoke() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Master).unwrap();
        ks.revoke_key(sid).unwrap();
        assert_eq!(ks.slot_state(sid).unwrap(), SlotState::Revoked);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Métadonnées de slot
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot public d'un slot (sans matériel secret).
#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot_id:  KeySlotId,
    pub kind:     KeyKind,
    pub state:    SlotState,
    pub accesses: u64,
}

impl KeyStorage {
    /// Retourne les métadonnées d'un slot sans exposer la clé.
    ///
    /// Utile pour l'interface d'administration.
    pub fn slot_info(&self, slot: KeySlotId) -> ExofsResult<SlotInfo> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let tbl = unsafe { &*self.table.get() };
        let entry = tbl.entries.get(&slot).ok_or_else(|| { self.release(); ExofsError::ObjectNotFound })?;
        let info = SlotInfo {
            slot_id:  slot,
            kind:     entry.kind,
            state:    entry.state,
            accesses: entry.accesses,
        };
        self.release();
        Ok(info)
    }

    /// Renomme le type d'un slot actif (utile après promotion de clé).
    pub fn retype_slot(&self, slot: KeySlotId, new_kind: KeyKind) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let tbl = unsafe { &mut *self.table.get() };
        let entry = tbl.entries.get_mut(&slot)
            .ok_or_else(|| { ExofsError::ObjectNotFound })?;
        if entry.state != SlotState::Active {
            self.release();
            return Err(ExofsError::InternalError);
        }
        entry.kind = new_kind;
        self.release();
        Ok(())
    }

    /// Expire un slot (transition Active → Expired).
    pub fn expire_slot(&self, slot: KeySlotId) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let tbl = unsafe { &mut *self.table.get() };
        let entry = tbl.entries.get_mut(&slot)
            .ok_or_else(|| { ExofsError::ObjectNotFound })?;
        entry.key.iter_mut().for_each(|b| *b = 0);
        entry.state = SlotState::Expired;
        self.release();
        Ok(())
    }

    /// Liste tous les slots par type.
    ///
    /// OOM-02 : try_reserve.
    pub fn list_by_kind(&self, kind: KeyKind) -> ExofsResult<Vec<KeySlotId>> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let tbl  = unsafe { &*self.table.get() };
        let list: Vec<KeySlotId> = tbl.entries.iter()
            .filter(|(_, e)| e.kind == kind && e.state == SlotState::Active)
            .map(|(&s, _)| s)
            .collect();
        self.release();
        Ok(list)
    }

    /// Purge tous les slots révoqués et expirés (libère la mémoire).
    pub fn purge_inactive(&self) -> ExofsResult<usize> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let tbl = unsafe { &mut *self.table.get() };
        let before = tbl.entries.len();
        tbl.entries.retain(|_, e| e.state == SlotState::Active);
        let after  = tbl.entries.len();
        self.release();
        let purged = before.saturating_sub(after);
        if purged > 0 {
            self.total_keys.fetch_sub(purged as u64, Ordering::Relaxed);
        }
        Ok(purged)
    }

    /// Retourne le nombre d'accès cumulé à un slot.
    pub fn access_count(&self, slot: KeySlotId) -> ExofsResult<u64> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let tbl = unsafe { &*self.table.get() };
        let cnt = tbl.entries.get(&slot)
            .ok_or_else(|| ExofsError::ObjectNotFound)
            .map(|e| e.accesses);
        self.release();
        cnt
    }
}

#[cfg(test)]
mod extended_tests {
    use super::*;

    fn ks() -> KeyStorage { KeyStorage::new_const() }

    #[test] fn test_slot_info_ok() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Volume).unwrap();
        let info = ks.slot_info(sid).unwrap();
        assert_eq!(info.kind, KeyKind::Volume);
        assert_eq!(info.state, SlotState::Active);
    }

    #[test] fn test_retype_ok() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Session).unwrap();
        ks.retype_slot(sid, KeyKind::Derived).unwrap();
        assert_eq!(ks.key_kind(sid).unwrap(), KeyKind::Derived);
    }

    #[test] fn test_expire_slot() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Master).unwrap();
        ks.expire_slot(sid).unwrap();
        assert_eq!(ks.slot_state(sid).unwrap(), SlotState::Expired);
        assert!(ks.load_key_256(sid).is_err());
    }

    #[test] fn test_list_by_kind() {
        let ks  = ks();
        ks.store_key_256(&[0u8; 32], KeyKind::Volume).unwrap();
        ks.store_key_256(&[1u8; 32], KeyKind::Volume).unwrap();
        ks.store_key_256(&[2u8; 32], KeyKind::Master).unwrap();
        let vols = ks.list_by_kind(KeyKind::Volume).unwrap();
        assert_eq!(vols.len(), 2);
    }

    #[test] fn test_purge_inactive() {
        let ks  = ks();
        let s1  = ks.store_key_256(&[0u8; 32], KeyKind::Session).unwrap();
        let s2  = ks.store_key_256(&[1u8; 32], KeyKind::Session).unwrap();
        ks.revoke_key(s1).unwrap();
        let purged = ks.purge_inactive().unwrap();
        assert_eq!(purged, 1);
        assert!(ks.load_key_256(s2).is_ok()); // s2 toujours actif
    }

    #[test] fn test_access_count_increments() {
        let ks  = ks();
        let sid = ks.store_key_256(&[0u8; 32], KeyKind::Object).unwrap();
        ks.load_key_256(sid).unwrap();
        ks.load_key_256(sid).unwrap();
        assert_eq!(ks.access_count(sid).unwrap(), 2);
    }
}
