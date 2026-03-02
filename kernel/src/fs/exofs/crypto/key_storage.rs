//! KeyStorage — stockage sécurisé des clés en mémoire kernel (no_std).
//!
//! Toutes les clés sont zeroize-on-drop. La table est protégée par SpinLock.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;

/// Identifiant de slot dans le KeyStorage.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct KeySlotId(pub u64);

/// Type de clé stockée.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyKind {
    Master,
    Volume,
    Object,
    Derived,
}

/// Payload d'une clé (256-bit maximum).
struct KeyPayload {
    bytes:     [u8; 32],
    len:       usize,     // Longueur effective (32 pour 256-bit).
    kind:      KeyKind,
    slot_id:   KeySlotId,
    use_count: u64,
}

impl Drop for KeyPayload {
    fn drop(&mut self) {
        // Zeroize.
        self.bytes.iter_mut().for_each(|b| *b = 0);
        self.len = 0;
    }
}

/// Stockage global des clés.
pub static KEY_STORAGE: KeyStorage = KeyStorage::new_const();

pub struct KeyStorage {
    table:      SpinLock<StorageTable>,
    next_slot:  AtomicU64,
    total_keys: AtomicU64,
}

struct StorageTable {
    slots: BTreeMap<KeySlotId, Box<KeyPayload>>,
}

impl KeyStorage {
    pub const fn new_const() -> Self {
        Self {
            table:      SpinLock::new(StorageTable { slots: BTreeMap::new() }),
            next_slot:  AtomicU64::new(1),
            total_keys: AtomicU64::new(0),
        }
    }

    /// Stocke une clé de 32 bytes et retourne son KeySlotId.
    pub fn store_key_256(
        &self,
        bytes: &[u8; 32],
        kind: KeyKind,
    ) -> Result<KeySlotId, FsError> {
        let slot_id = KeySlotId(self.next_slot.fetch_add(1, Ordering::SeqCst));
        let payload = Box::new(KeyPayload {
            bytes: *bytes,
            len: 32,
            kind,
            slot_id,
            use_count: 0,
        });
        let mut table = self.table.lock();
        table.slots.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        table.slots.insert(slot_id, payload);
        self.total_keys.fetch_add(1, Ordering::Relaxed);
        Ok(slot_id)
    }

    /// Accède à une clé en lecture (copie des bytes dans `out`).
    pub fn load_key_256(
        &self,
        slot: KeySlotId,
        out: &mut [u8; 32],
    ) -> Result<(), FsError> {
        let mut table = self.table.lock();
        let entry = table.slots.get_mut(&slot).ok_or(FsError::NotFound)?;
        if entry.len != 32 { return Err(FsError::InvalidData); }
        out.copy_from_slice(&entry.bytes);
        entry.use_count = entry.use_count.saturating_add(1);
        Ok(())
    }

    /// Révoque (efface) un slot de clé.
    pub fn revoke_key(&self, slot: KeySlotId) -> Result<(), FsError> {
        let mut table = self.table.lock();
        let removed = table.slots.remove(&slot);
        if removed.is_none() { return Err(FsError::NotFound); }
        // Drop déclenche le zeroize.
        self.total_keys.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    /// Retourne le nombre de clés actuellement stockées.
    pub fn total_keys(&self) -> u64 {
        self.total_keys.load(Ordering::Relaxed)
    }

    /// Liste les slot IDs d'un type donné.
    pub fn list_by_kind(
        &self,
        kind: KeyKind,
    ) -> Result<alloc::vec::Vec<KeySlotId>, FsError> {
        let table = self.table.lock();
        let mut out = alloc::vec::Vec::new();
        for (id, p) in &table.slots {
            if p.kind == kind {
                out.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                out.push(*id);
            }
        }
        Ok(out)
    }

    /// Revoque toutes les clés d'un type donné (ex : rotation, démontage volume).
    pub fn revoke_all_by_kind(&self, kind: KeyKind) {
        let mut table = self.table.lock();
        let to_remove: alloc::vec::Vec<KeySlotId> = table
            .slots
            .iter()
            .filter(|(_, v)| v.kind == kind)
            .map(|(k, _)| *k)
            .collect();
        for slot in to_remove {
            table.slots.remove(&slot);
            self.total_keys.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Zeroize et vide tout le stockage (démontage d'urgence).
    pub fn clear_all(&self) {
        let mut table = self.table.lock();
        let cnt = table.slots.len() as u64;
        table.slots.clear(); // Drop déclenche le zeroize sur chaque payload.
        self.total_keys.fetch_sub(cnt, Ordering::Relaxed);
    }
}
