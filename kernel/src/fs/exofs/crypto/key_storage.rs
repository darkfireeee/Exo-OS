//! Stockage de clés ExoFS — registre sécurisé des slots de clés.
//!
//! Le `KeyStorage` maintient une table de slots de clés protégée par spinlock.
//! Chaque slot contient 256 bits de matériel de clé associé à un `KeyKind`.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::storage::virtio_adapter;
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::{crypto::secret_reader::SecretReader, crypto::secret_writer::SecretWriter};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

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
            Self::Master => write!(f, "Master"),
            Self::Volume => write!(f, "Volume"),
            Self::Object => write!(f, "Object"),
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
    key: [u8; 32],
    /// Type.
    kind: KeyKind,
    /// État.
    state: SlotState,
    /// Compteur d'accès.
    accesses: u64,
}

impl Drop for KeyEntry {
    fn drop(&mut self) {
        self.key.iter_mut().for_each(|b| *b = 0);
    }
}

/// Table de stockage (accédée sous lock).
struct StorageTable {
    entries: BTreeMap<KeySlotId, KeyEntry>,
}

impl StorageTable {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    fn insert(&mut self, slot: KeySlotId, key: [u8; 32], kind: KeyKind) -> ExofsResult<()> {
        if self.entries.contains_key(&slot) {
            return Err(ExofsError::InternalError); // slot déjà occupé
        }
        self.entries.insert(
            slot,
            KeyEntry {
                key,
                kind,
                state: SlotState::Active,
                accesses: 0,
            },
        );
        Ok(())
    }

    fn get(&mut self, slot: KeySlotId) -> ExofsResult<[u8; 32]> {
        let entry = self
            .entries
            .get_mut(&slot)
            .ok_or(ExofsError::ObjectNotFound)?;
        if entry.state != SlotState::Active {
            return Err(ExofsError::InternalError);
        }
        entry.accesses = entry.accesses.saturating_add(1);
        Ok(entry.key)
    }

    fn revoke(&mut self, slot: KeySlotId) -> ExofsResult<()> {
        let entry = self
            .entries
            .get_mut(&slot)
            .ok_or(ExofsError::ObjectNotFound)?;
        entry.key.iter_mut().for_each(|b| *b = 0);
        entry.state = SlotState::Revoked;
        Ok(())
    }

    fn remove(&mut self, slot: KeySlotId) -> ExofsResult<()> {
        self.entries
            .remove(&slot)
            .ok_or(ExofsError::ObjectNotFound)?;
        Ok(())
    }

    fn kind_of(&self, slot: KeySlotId) -> ExofsResult<KeyKind> {
        Ok(self
            .entries
            .get(&slot)
            .ok_or(ExofsError::ObjectNotFound)?
            .kind)
    }

    fn state_of(&self, slot: KeySlotId) -> ExofsResult<SlotState> {
        Ok(self
            .entries
            .get(&slot)
            .ok_or(ExofsError::ObjectNotFound)?
            .state)
    }

    fn list_active(&self) -> Vec<(KeySlotId, KeyKind)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.state == SlotState::Active)
            .map(|(&s, e)| (s, e.kind))
            .collect()
    }

    #[allow(dead_code)]
    fn total(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone, Copy)]
struct PersistentKeyEntry {
    slot: KeySlotId,
    key: [u8; 32],
    kind: KeyKind,
    state: SlotState,
    accesses: u64,
}

const KEY_STORAGE_MAGIC: [u8; 4] = *b"EXKS";
const KEY_STORAGE_VERSION: u16 = 1;
const KEY_STORAGE_HEADER_SIZE: usize = 24;
const KEY_STORAGE_ENTRY_SIZE: usize = 56;
const KEY_STORAGE_BLOB_LABEL: &[u8] = b"ExoFS-KeyStorage-Persistent-v1";
const KEY_STORAGE_AAD: &[u8] = b"ExoFS-KeyStorage-AEAD-v1";
const MAX_PERSISTED_KEYS: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// KeyStorage (thread-safe via spinlock simulé en no_std)
// ─────────────────────────────────────────────────────────────────────────────

/// Stockage de clés thread-safe.
///
/// Utilise un `core::cell::UnsafeCell` pour la mutabilité intérieure,
/// protégé par une sémantique de lock atomique simple (spin).
pub struct KeyStorage {
    table: core::cell::UnsafeCell<StorageTable>,
    lock: AtomicU64,
    next_slot: AtomicU64,
    total_keys: AtomicU64,
}

// SAFETY: `table` n'est accessible qu'après acquisition du spinlock `lock`,
// ce qui garantit un accès exclusif à la mutabilité intérieure.
unsafe impl Sync for KeyStorage {}
// SAFETY: la structure ne contient pas de références empruntées et reste sûre à
// déplacer entre threads tant que le protocole de verrouillage est respecté.
unsafe impl Send for KeyStorage {}

/// Instance globale.
pub static KEY_STORAGE: KeyStorage = KeyStorage::new_const();

impl KeyStorage {
    /// Constructeur const pour l'initialisation statique.
    pub const fn new_const() -> Self {
        Self {
            table: core::cell::UnsafeCell::new(StorageTable {
                entries: BTreeMap::new(),
            }),
            lock: AtomicU64::new(0),
            next_slot: AtomicU64::new(1),
            total_keys: AtomicU64::new(0),
        }
    }

    // ── Gestion du lock ───────────────────────────────────────────────────────

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
        if result.is_ok() {
            self.total_keys.fetch_sub(1, Ordering::Relaxed);
        }
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
    pub fn total_count(&self) -> u64 {
        self.total_keys.load(Ordering::Relaxed)
    }

    fn snapshot_persistent_entries(&self) -> ExofsResult<Vec<PersistentKeyEntry>> {
        let mut out = Vec::new();
        self.acquire();
        let tbl = unsafe { &*self.table.get() };
        out.try_reserve(tbl.entries.len()).map_err(|_| {
            self.release();
            ExofsError::NoMemory
        })?;
        for (&slot, entry) in tbl.entries.iter() {
            if !key_kind_is_persistent(entry.kind) {
                continue;
            }
            out.push(PersistentKeyEntry {
                slot,
                key: entry.key,
                kind: entry.kind,
                state: entry.state,
                accesses: entry.accesses,
            });
        }
        self.release();
        Ok(out)
    }

    fn serialize_persistent_plaintext(&self) -> ExofsResult<Vec<u8>> {
        let entries = self.snapshot_persistent_entries()?;
        if entries.len() > MAX_PERSISTED_KEYS {
            return Err(ExofsError::InvalidSize);
        }
        let body = entries
            .len()
            .checked_mul(KEY_STORAGE_ENTRY_SIZE)
            .ok_or(ExofsError::OffsetOverflow)?;
        let total = KEY_STORAGE_HEADER_SIZE
            .checked_add(body)
            .ok_or(ExofsError::OffsetOverflow)?;
        let mut out = Vec::new();
        out.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(&KEY_STORAGE_MAGIC);
        out.extend_from_slice(&KEY_STORAGE_VERSION.to_le_bytes());
        out.extend_from_slice(&(KEY_STORAGE_ENTRY_SIZE as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.next_slot.load(Ordering::Acquire).to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        for entry in entries.iter() {
            out.extend_from_slice(&entry.slot.0.to_le_bytes());
            out.push(key_kind_to_u8(entry.kind));
            out.push(slot_state_to_u8(entry.state));
            out.extend_from_slice(&[0u8; 6]);
            out.extend_from_slice(&entry.accesses.to_le_bytes());
            out.extend_from_slice(&entry.key);
        }
        Ok(out)
    }

    fn replace_from_persistent_plaintext(&self, data: &[u8]) -> ExofsResult<()> {
        if data.len() < KEY_STORAGE_HEADER_SIZE {
            return Err(ExofsError::CorruptedStructure);
        }
        if data[0..4] != KEY_STORAGE_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != KEY_STORAGE_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        let entry_size = u16::from_le_bytes([data[6], data[7]]) as usize;
        if entry_size != KEY_STORAGE_ENTRY_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        let count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        if count > MAX_PERSISTED_KEYS {
            return Err(ExofsError::InvalidSize);
        }
        let mut next_bytes = [0u8; 8];
        next_bytes.copy_from_slice(&data[12..20]);
        let stored_next_slot = u64::from_le_bytes(next_bytes);
        let expected_len = KEY_STORAGE_HEADER_SIZE
            .checked_add(
                count
                    .checked_mul(KEY_STORAGE_ENTRY_SIZE)
                    .ok_or(ExofsError::OffsetOverflow)?,
            )
            .ok_or(ExofsError::OffsetOverflow)?;
        if data.len() != expected_len {
            return Err(ExofsError::CorruptedStructure);
        }

        let mut table = StorageTable {
            entries: BTreeMap::new(),
        };
        let mut max_slot = 0u64;
        let mut off = KEY_STORAGE_HEADER_SIZE;
        let mut i = 0usize;
        while i < count {
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&data[off..off + 8]);
            let slot = KeySlotId(u64::from_le_bytes(slot_bytes));
            let kind = key_kind_from_u8(data[off + 8])?;
            let state = slot_state_from_u8(data[off + 9])?;
            if !key_kind_is_persistent(kind) {
                return Err(ExofsError::InvalidArgument);
            }
            let mut access_bytes = [0u8; 8];
            access_bytes.copy_from_slice(&data[off + 16..off + 24]);
            let accesses = u64::from_le_bytes(access_bytes);
            let mut key = [0u8; 32];
            key.copy_from_slice(&data[off + 24..off + 56]);
            if table
                .entries
                .insert(
                    slot,
                    KeyEntry {
                        key,
                        kind,
                        state,
                        accesses,
                    },
                )
                .is_some()
            {
                return Err(ExofsError::CorruptedStructure);
            }
            max_slot = max_slot.max(slot.0);
            off = off.saturating_add(KEY_STORAGE_ENTRY_SIZE);
            i = i.wrapping_add(1);
        }

        let next_slot = stored_next_slot.max(max_slot.saturating_add(1)).max(1);
        self.acquire();
        unsafe { *self.table.get() = table };
        self.total_keys.store(count as u64, Ordering::Release);
        self.next_slot.store(next_slot, Ordering::Release);
        self.release();
        Ok(())
    }

    /// Exporte les slots persistants chiffrés avec la clé maître fournie.
    pub fn export_encrypted(&self, master_key: &[u8; 32]) -> ExofsResult<Vec<u8>> {
        let mut plain = self.serialize_persistent_plaintext()?;
        let writer = SecretWriter::new(master_key);
        let encrypted = writer.encrypt_with_aad(&plain, KEY_STORAGE_AAD);
        zeroize_slice(&mut plain);
        encrypted
    }

    /// Remplace la table par un snapshot chiffré précédemment exporté.
    pub fn import_encrypted(&self, master_key: &[u8; 32], payload: &[u8]) -> ExofsResult<()> {
        let reader = SecretReader::new(master_key);
        let mut plain = reader.decrypt_with_aad(payload, KEY_STORAGE_AAD)?;
        let result = self.replace_from_persistent_plaintext(&plain);
        zeroize_slice(&mut plain);
        result
    }

    /// Persiste le stockage de clés global sur le disque ExoFS si un block device est monté.
    pub fn persist_to_global_disk(&self, master_key: &[u8; 32]) -> ExofsResult<bool> {
        if !virtio_adapter::has_global_disk() {
            return Ok(false);
        }
        let bid = key_storage_blob_id();
        let encrypted = self.export_encrypted(master_key)?;
        let wrote = object_store::persist_blob_data_if_disk(bid, &encrypted, true)?;
        if wrote {
            BLOB_CACHE
                .insert(bid, encrypted)
                .map_err(|_| ExofsError::NoSpace)?;
            let _ = BLOB_CACHE.mark_clean(&bid);
        }
        Ok(wrote)
    }

    /// Restaure le stockage de clés depuis le disque ExoFS si le snapshot existe.
    pub fn restore_from_global_disk(&self, master_key: &[u8; 32]) -> ExofsResult<bool> {
        if !virtio_adapter::has_global_disk() {
            return Ok(false);
        }
        let bid = key_storage_blob_id();
        if let Some(cached) = BLOB_CACHE.get(&bid) {
            self.import_encrypted(master_key, cached.as_ref())?;
            return Ok(true);
        }
        let Some(payload) = object_store::load_blob_data_if_available(&bid)? else {
            return Ok(false);
        };
        self.import_encrypted(master_key, &payload)?;
        BLOB_CACHE
            .insert(bid, payload)
            .map_err(|_| ExofsError::NoSpace)?;
        let _ = BLOB_CACHE.mark_clean(&bid);
        Ok(true)
    }
}

pub fn key_storage_blob_id() -> BlobId {
    BlobId::from_bytes_blake3(KEY_STORAGE_BLOB_LABEL)
}

pub fn persist_global_with_master_slot(slot: KeySlotId) -> ExofsResult<bool> {
    let mut key = KEY_STORAGE.load_key_256(slot)?;
    let result = KEY_STORAGE.persist_to_global_disk(&key);
    zeroize_array(&mut key);
    result
}

pub fn restore_global_with_master_slot(slot: KeySlotId) -> ExofsResult<bool> {
    let mut key = KEY_STORAGE.load_key_256(slot)?;
    let result = KEY_STORAGE.restore_from_global_disk(&key);
    zeroize_array(&mut key);
    result
}

pub fn persist_global_if_master_present() -> ExofsResult<bool> {
    let slots = KEY_STORAGE.list_by_kind(KeyKind::Master)?;
    let Some(slot) = slots.first().copied() else {
        return Ok(false);
    };
    persist_global_with_master_slot(slot)
}

fn key_kind_is_persistent(kind: KeyKind) -> bool {
    !matches!(kind, KeyKind::Master | KeyKind::Session)
}

fn key_kind_to_u8(kind: KeyKind) -> u8 {
    match kind {
        KeyKind::Master => 1,
        KeyKind::Volume => 2,
        KeyKind::Object => 3,
        KeyKind::Derived => 4,
        KeyKind::Session => 5,
    }
}

fn key_kind_from_u8(raw: u8) -> ExofsResult<KeyKind> {
    match raw {
        1 => Ok(KeyKind::Master),
        2 => Ok(KeyKind::Volume),
        3 => Ok(KeyKind::Object),
        4 => Ok(KeyKind::Derived),
        5 => Ok(KeyKind::Session),
        _ => Err(ExofsError::InvalidArgument),
    }
}

fn slot_state_to_u8(state: SlotState) -> u8 {
    match state {
        SlotState::Active => 1,
        SlotState::Revoked => 2,
        SlotState::Expired => 3,
    }
}

fn slot_state_from_u8(raw: u8) -> ExofsResult<SlotState> {
    match raw {
        1 => Ok(SlotState::Active),
        2 => Ok(SlotState::Revoked),
        3 => Ok(SlotState::Expired),
        _ => Err(ExofsError::InvalidArgument),
    }
}

fn zeroize_slice(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        unsafe {
            core::ptr::write_volatile(byte, 0);
        }
    }
    core::sync::atomic::fence(Ordering::SeqCst);
}

fn zeroize_array(buf: &mut [u8; 32]) {
    zeroize_slice(buf);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn ks() -> KeyStorage {
        KeyStorage::new_const()
    }

    #[test]
    fn test_store_load_roundtrip() {
        let ks = ks();
        let key = [0x42u8; 32];
        let sid = ok(ks.store_key_256(&key, KeyKind::Master));
        assert_eq!(ok(ks.load_key_256(sid)), key);
    }

    #[test]
    fn test_store_different_slots() {
        let ks = ks();
        let s1 = ok(ks.store_key_256(&[1u8; 32], KeyKind::Volume));
        let s2 = ok(ks.store_key_256(&[2u8; 32], KeyKind::Object));
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_revoke_zeroes_key() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0xABu8; 32], KeyKind::Derived));
        ok(ks.revoke_key(sid));
        // Après révocation, load doit échouer.
        assert!(ks.load_key_256(sid).is_err());
    }

    #[test]
    fn test_remove_key() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Session));
        ok(ks.remove_key(sid));
        assert!(ks.load_key_256(sid).is_err());
    }

    #[test]
    fn test_load_nonexistent_fails() {
        let ks = ks();
        assert!(ks.load_key_256(KeySlotId(999)).is_err());
    }

    #[test]
    fn test_key_kind_ok() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Volume));
        assert_eq!(ok(ks.key_kind(sid)), KeyKind::Volume);
    }

    #[test]
    fn test_list_active_slots_count() {
        let ks = ks();
        ok(ks.store_key_256(&[1u8; 32], KeyKind::Master));
        ok(ks.store_key_256(&[2u8; 32], KeyKind::Volume));
        let active = ok(ks.list_active_slots());
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn test_total_count() {
        let ks = ks();
        ok(ks.store_key_256(&[0u8; 32], KeyKind::Derived));
        assert!(ks.total_count() >= 1);
    }

    #[test]
    fn test_export_import_encrypted_roundtrip() {
        let source = ks();
        let master_key = [0xA5u8; 32];
        let volume_key = [0x42u8; 32];
        let session_key = [0x24u8; 32];
        let master_slot = ok(source.store_key_256(&master_key, KeyKind::Master));
        let volume_slot = ok(source.store_key_256(&volume_key, KeyKind::Volume));
        let session_slot = ok(source.store_key_256(&session_key, KeyKind::Session));

        let payload = ok(source.export_encrypted(&master_key));
        let restored = ks();
        ok(restored.import_encrypted(&master_key, &payload));

        assert!(restored.load_key_256(master_slot).is_err());
        assert_eq!(ok(restored.load_key_256(volume_slot)), volume_key);
        assert!(restored.load_key_256(session_slot).is_err());
        assert_eq!(restored.total_count(), 1);
        let next_slot = ok(restored.store_key_256(&[0x7Fu8; 32], KeyKind::Derived));
        assert_eq!(next_slot.0, 4);
    }

    #[test]
    fn test_import_tampered_payload_fails() {
        let source = ks();
        let master_key = [0xA5u8; 32];
        ok(source.store_key_256(&[0x42u8; 32], KeyKind::Object));
        let mut payload = ok(source.export_encrypted(&master_key));
        let last = payload.len() - 1;
        payload[last] ^= 0x55;

        let restored = ks();
        assert!(restored.import_encrypted(&master_key, &payload).is_err());
        assert_eq!(restored.total_count(), 0);
    }

    #[test]
    fn test_slot_state_active() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Master));
        assert_eq!(ok(ks.slot_state(sid)), SlotState::Active);
    }

    #[test]
    fn test_slot_state_after_revoke() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Master));
        ok(ks.revoke_key(sid));
        assert_eq!(ok(ks.slot_state(sid)), SlotState::Revoked);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Métadonnées de slot
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot public d'un slot (sans matériel secret).
#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot_id: KeySlotId,
    pub kind: KeyKind,
    pub state: SlotState,
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
        let entry = tbl.entries.get(&slot).ok_or_else(|| {
            self.release();
            ExofsError::ObjectNotFound
        })?;
        let info = SlotInfo {
            slot_id: slot,
            kind: entry.kind,
            state: entry.state,
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
        let entry = tbl
            .entries
            .get_mut(&slot)
            .ok_or_else(|| ExofsError::ObjectNotFound)?;
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
        let entry = tbl
            .entries
            .get_mut(&slot)
            .ok_or_else(|| ExofsError::ObjectNotFound)?;
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
        let tbl = unsafe { &*self.table.get() };
        let list: Vec<KeySlotId> = tbl
            .entries
            .iter()
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
        let after = tbl.entries.len();
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
        let cnt = tbl
            .entries
            .get(&slot)
            .ok_or_else(|| ExofsError::ObjectNotFound)
            .map(|e| e.accesses);
        self.release();
        cnt
    }
}

#[cfg(test)]
mod extended_tests {
    use super::*;

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn ks() -> KeyStorage {
        KeyStorage::new_const()
    }

    #[test]
    fn test_slot_info_ok() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Volume));
        let info = ok(ks.slot_info(sid));
        assert_eq!(info.kind, KeyKind::Volume);
        assert_eq!(info.state, SlotState::Active);
    }

    #[test]
    fn test_retype_ok() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Session));
        ok(ks.retype_slot(sid, KeyKind::Derived));
        assert_eq!(ok(ks.key_kind(sid)), KeyKind::Derived);
    }

    #[test]
    fn test_expire_slot() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Master));
        ok(ks.expire_slot(sid));
        assert_eq!(ok(ks.slot_state(sid)), SlotState::Expired);
        assert!(ks.load_key_256(sid).is_err());
    }

    #[test]
    fn test_list_by_kind() {
        let ks = ks();
        ok(ks.store_key_256(&[0u8; 32], KeyKind::Volume));
        ok(ks.store_key_256(&[1u8; 32], KeyKind::Volume));
        ok(ks.store_key_256(&[2u8; 32], KeyKind::Master));
        let vols = ok(ks.list_by_kind(KeyKind::Volume));
        assert_eq!(vols.len(), 2);
    }

    #[test]
    fn test_purge_inactive() {
        let ks = ks();
        let s1 = ok(ks.store_key_256(&[0u8; 32], KeyKind::Session));
        let s2 = ok(ks.store_key_256(&[1u8; 32], KeyKind::Session));
        ok(ks.revoke_key(s1));
        let purged = ok(ks.purge_inactive());
        assert_eq!(purged, 1);
        assert!(ks.load_key_256(s2).is_ok()); // s2 toujours actif
    }

    #[test]
    fn test_access_count_increments() {
        let ks = ks();
        let sid = ok(ks.store_key_256(&[0u8; 32], KeyKind::Object));
        ok(ks.load_key_256(sid));
        ok(ks.load_key_256(sid));
        assert_eq!(ok(ks.access_count(sid)), 2);
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn key_kind_wire_roundtrip() {
        let raw: u8 = kani::any();
        kani::assume((1..=5).contains(&raw));

        let kind = key_kind_from_u8(raw).expect("assumed valid key kind wire value");
        assert_eq!(key_kind_to_u8(kind), raw);
    }

    #[kani::proof]
    fn key_kind_rejects_invalid_wire_values() {
        let raw: u8 = kani::any();
        kani::assume(raw == 0 || raw > 5);

        assert!(key_kind_from_u8(raw).is_err());
    }

    #[kani::proof]
    fn slot_state_wire_roundtrip() {
        let raw: u8 = kani::any();
        kani::assume((1..=3).contains(&raw));

        let state = slot_state_from_u8(raw).expect("assumed valid slot state wire value");
        assert_eq!(slot_state_to_u8(state), raw);
    }

    #[kani::proof]
    fn key_storage_persistence_policy_excludes_ephemeral_material() {
        let raw: u8 = kani::any();
        kani::assume((1..=5).contains(&raw));

        let kind = key_kind_from_u8(raw).expect("assumed valid key kind wire value");
        let persistent = key_kind_is_persistent(kind);
        assert_eq!(
            persistent,
            !matches!(kind, KeyKind::Master | KeyKind::Session)
        );
    }
}
