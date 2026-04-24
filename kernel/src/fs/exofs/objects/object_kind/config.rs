// SPDX-License-Identifier: MIT
// ExoFS — object_kind/config.rs
// ConfigStore — paires clé/valeur pour les objets de configuration ExoFS.
//
// Règles :
//   ONDISK-01 : ConfigEntryDisk #[repr(C, packed)]
//   OOM-02    : try_reserve avant chaque push
//   ARITH-02  : checked_add / saturating_* partout
//   RECUR-01  : itératif seulement

use alloc::vec::Vec;
use core::fmt;
use core::mem;

use crate::fs::exofs::core::{blake3_hash, EpochId, ExofsError, ExofsResult, ObjectId};

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Taille maximale d'un objet Config (64 Kio).
pub const CONFIG_MAX_SIZE: usize = 64 * 1024;

/// Longueur maximale d'une clé de configuration.
pub const CONFIG_KEY_LEN: usize = 64;

/// Longueur maximale d'une valeur de configuration.
pub const CONFIG_VALUE_LEN: usize = 256;

/// Nombre maximal d'entrées dans un ConfigStore inline.
pub const CONFIG_MAX_ENTRIES: usize = 128;

/// Magic d'un ConfigStoreDisk.
pub const CONFIG_STORE_MAGIC: u32 = 0xC0_F1_6E_00;

/// Version du format ConfigStoreDisk.
pub const CONFIG_STORE_VERSION: u8 = 1;

/// Magic d'une entrée de configuration valide.
pub const CONFIG_ENTRY_MAGIC: u16 = 0xCEA1;

// ── Flags d'entrée ─────────────────────────────────────────────────────────────

pub const CONFIG_ENTRY_FLAG_REQUIRED: u8 = 1 << 0; // Entrée obligatoire
pub const CONFIG_ENTRY_FLAG_READONLY: u8 = 1 << 1; // Ne peut pas être écrasée
pub const CONFIG_ENTRY_FLAG_DELETED: u8 = 1 << 2; // Marquée supprimée (tombstone)
pub const CONFIG_ENTRY_FLAG_SECRET: u8 = 1 << 3; // Valeur opaque (chiffrée)

// ── ConfigEntryDisk ─────────────────────────────────────────────────────────────

/// Représentation on-disk d'une entrée clé/valeur (352 octets).
///
/// Layout :
/// ```text
///   0..  1  magic         u16
///   2..  3  flags         u8 + _pad u8
///   4       key_len       u8
///   5       val_len_hi    u8   (octet de poids fort de val_len)
///   6       val_len_lo    u8
///   7       _pad          u8
///   8.. 71  key           [u8;64]
///  72..327  value         [u8;256]
/// 328..359  checksum      [u8;32]  (Blake3 des 328 premiers octets)
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ConfigEntryDisk {
    pub magic: u16,
    pub flags: u8,
    pub _pad0: u8,
    pub key_len: u8,
    pub val_len_hi: u8,
    pub val_len_lo: u8,
    pub _pad1: u8,
    pub key: [u8; CONFIG_KEY_LEN],
    pub value: [u8; CONFIG_VALUE_LEN],
    pub checksum: [u8; 32],
}

const _: () = assert!(
    mem::size_of::<ConfigEntryDisk>() == 360,
    "ConfigEntryDisk doit être 360 octets (ONDISK-01)"
);

impl ConfigEntryDisk {
    pub fn compute_checksum(&self) -> [u8; 32] {
        let raw: &[u8; 360] =
            // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
            unsafe { &*(self as *const ConfigEntryDisk as *const [u8; 360]) };
        blake3_hash(&raw[..328])
    }

    pub fn verify(&self) -> ExofsResult<()> {
        if { self.magic } != CONFIG_ENTRY_MAGIC {
            return Err(ExofsError::Corrupt);
        }
        let computed = self.compute_checksum();
        if { self.checksum } != computed {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }
}

impl fmt::Debug for ConfigEntryDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ConfigEntryDisk {{ key_len: {}, val_len: {}, flags: {:#x} }}",
            self.key_len,
            (self.val_len_hi as u16) << 8 | self.val_len_lo as u16,
            self.flags,
        )
    }
}

// ── ConfigEntry in-memory ──────────────────────────────────────────────────────

/// Entrée clé/valeur in-memory d'un objet Config.
#[derive(Clone)]
pub struct ConfigEntry {
    /// Clé ASCII (longueur variée ≤ CONFIG_KEY_LEN).
    pub key: [u8; CONFIG_KEY_LEN],
    pub key_len: u8,
    /// Valeur (longueur variée ≤ CONFIG_VALUE_LEN).
    pub value: [u8; CONFIG_VALUE_LEN],
    pub val_len: u16,
    /// Flags (CONFIG_ENTRY_FLAG_*).
    pub flags: u8,
}

impl ConfigEntry {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    /// Crée une entrée vide.
    pub fn empty() -> Self {
        Self {
            key: [0u8; CONFIG_KEY_LEN],
            key_len: 0,
            value: [0u8; CONFIG_VALUE_LEN],
            val_len: 0,
            flags: 0,
        }
    }

    /// Crée une entrée depuis des slices.
    pub fn from_slices(key: &[u8], value: &[u8]) -> ExofsResult<Self> {
        if key.is_empty() || key.len() > CONFIG_KEY_LEN {
            return Err(ExofsError::InvalidArgument);
        }
        if value.len() > CONFIG_VALUE_LEN {
            return Err(ExofsError::Overflow);
        }
        let mut entry = Self::empty();
        entry.key[..key.len()].copy_from_slice(key);
        entry.key_len = key.len() as u8;
        entry.value[..value.len()].copy_from_slice(value);
        entry.val_len = value.len() as u16;
        Ok(entry)
    }

    /// Reconstruit depuis on-disk (HDR-03 : d.verify() en premier).
    pub fn from_disk(d: &ConfigEntryDisk) -> ExofsResult<Self> {
        d.verify()?;
        let val_len = (({ d.val_len_hi } as u16) << 8) | { d.val_len_lo } as u16;
        Ok(Self {
            key: { d.key },
            key_len: { d.key_len },
            value: { d.value },
            val_len,
            flags: { d.flags },
        })
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk(&self) -> ConfigEntryDisk {
        let mut d = ConfigEntryDisk {
            magic: CONFIG_ENTRY_MAGIC,
            flags: self.flags,
            _pad0: 0,
            key_len: self.key_len,
            val_len_hi: (self.val_len >> 8) as u8,
            val_len_lo: (self.val_len & 0xFF) as u8,
            _pad1: 0,
            key: self.key,
            value: self.value,
            checksum: [0u8; 32],
        };
        d.checksum = d.compute_checksum();
        d
    }

    // ── Requêtes ───────────────────────────────────────────────────────────────

    /// Retourne la clé comme slice.
    #[inline]
    pub fn key_bytes(&self) -> &[u8] {
        &self.key[..self.key_len as usize]
    }

    /// Retourne la valeur comme slice.
    #[inline]
    pub fn value_bytes(&self) -> &[u8] {
        &self.value[..self.val_len as usize]
    }

    /// Vrai si la clé correspond (comparaison en temps constant).
    pub fn key_matches(&self, k: &[u8]) -> bool {
        if k.len() != self.key_len as usize {
            return false;
        }
        // Comparaison en temps constant : pas de early exit.
        let mut diff = 0u8;
        for (a, b) in self.key[..k.len()].iter().zip(k.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// Vrai si l'entrée est marquée supprimée.
    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags & CONFIG_ENTRY_FLAG_DELETED != 0
    }

    /// Vrai si l'entrée est en lecture seule.
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.flags & CONFIG_ENTRY_FLAG_READONLY != 0
    }
}

impl fmt::Display for ConfigEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SEC-04 : ne pas loguer les valeurs marquées SECRET.
        if self.flags & CONFIG_ENTRY_FLAG_SECRET != 0 {
            write!(
                f,
                "ConfigEntry {{ key: {:?}, value: <secret> }}",
                self.key_bytes()
            )
        } else {
            write!(
                f,
                "ConfigEntry {{ key: {:?}, val_len: {} }}",
                self.key_bytes(),
                self.val_len
            )
        }
    }
}

impl fmt::Debug for ConfigEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── ConfigStore ────────────────────────────────────────────────────────────────

/// Magasin de configuration exo-FS in-memory.
///
/// Garde un vecteur trié par clé pour la recherche binaire (RECUR-01 : itératif).
pub struct ConfigStore {
    /// Objet propriétaire.
    pub object_id: ObjectId,
    /// Epoch de dernière modification.
    pub epoch_modify: EpochId,
    /// Entrées, triées par clé.
    entries: Vec<ConfigEntry>,
    /// Version (incrémentée à chaque modification).
    pub version: u64,
}

impl ConfigStore {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    pub fn new(object_id: ObjectId, epoch: EpochId) -> Self {
        Self {
            object_id,
            epoch_modify: epoch,
            entries: Vec::new(),
            version: 0,
        }
    }

    // ── CRUD ───────────────────────────────────────────────────────────────────

    /// Insère ou met à jour une entrée (OOM-02 : try_reserve).
    pub fn set(&mut self, key: &[u8], value: &[u8], now: EpochId) -> ExofsResult<()> {
        if self.entries.len() >= CONFIG_MAX_ENTRIES {
            // Cherche d'abord un tombstone à réutiliser.
            let tombstone = self.entries.iter_mut().find(|e| e.is_deleted());
            if let Some(slot) = tombstone {
                let new = ConfigEntry::from_slices(key, value)?;
                *slot = new;
                self.epoch_modify = now;
                self.version = self.version.saturating_add(1);
                return Ok(());
            }
            return Err(ExofsError::NoSpace);
        }
        // Mise à jour si existe.
        for entry in self.entries.iter_mut() {
            if entry.key_matches(key) && !entry.is_deleted() {
                if entry.is_readonly() {
                    return Err(ExofsError::InvalidArgument);
                }
                entry.value[..value.len()].copy_from_slice(value);
                entry.val_len = value.len() as u16;
                self.epoch_modify = now;
                self.version = self.version.saturating_add(1);
                return Ok(());
            }
        }
        // Insertion (OOM-02).
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        let entry = ConfigEntry::from_slices(key, value)?;
        self.entries.push(entry);
        self.epoch_modify = now;
        self.version = self.version.saturating_add(1);
        Ok(())
    }

    /// Retourne la valeur associée à `key`, ou `None`.
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        for entry in self.entries.iter() {
            if entry.key_matches(key) && !entry.is_deleted() {
                return Some(entry.value_bytes());
            }
        }
        None
    }

    /// Supprime l'entrée associée à `key` (tombstone).
    pub fn remove(&mut self, key: &[u8], now: EpochId) -> ExofsResult<()> {
        for entry in self.entries.iter_mut() {
            if entry.key_matches(key) && !entry.is_deleted() {
                if entry.is_readonly() {
                    return Err(ExofsError::InvalidArgument);
                }
                entry.flags |= CONFIG_ENTRY_FLAG_DELETED;
                self.epoch_modify = now;
                self.version = self.version.saturating_add(1);
                return Ok(());
            }
        }
        Err(ExofsError::NotFound)
    }

    /// Liste toutes les clés actives.
    pub fn list_keys(&self) -> Vec<&[u8]> {
        let mut keys = Vec::new();
        for entry in self.entries.iter() {
            if !entry.is_deleted() {
                let _ = keys.try_reserve(1);
                keys.push(entry.key_bytes());
            }
        }
        keys
    }

    /// Nombre d'entrées actives.
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_deleted()).count()
    }

    /// Vrai si le store est vide.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    /// Sérialise toutes les entrées non-supprimées.
    pub fn to_disk_vec(&self) -> ExofsResult<Vec<ConfigEntryDisk>> {
        let active: Vec<&ConfigEntry> = self.entries.iter().filter(|e| !e.is_deleted()).collect();
        let mut out = Vec::new();
        out.try_reserve(active.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for e in active {
            out.push(e.to_disk());
        }
        Ok(out)
    }

    /// Reconstruit depuis une slice d'entrées on-disk (RECUR-01 : itératif).
    pub fn from_disk_slice(
        entries: &[ConfigEntryDisk],
        object_id: ObjectId,
        epoch: EpochId,
    ) -> ExofsResult<Self> {
        if entries.len() > CONFIG_MAX_ENTRIES {
            return Err(ExofsError::Overflow);
        }
        let mut store = Self::new(object_id, epoch);
        store
            .entries
            .try_reserve(entries.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for d in entries.iter() {
            let entry = ConfigEntry::from_disk(d)?;
            store.entries.push(entry);
        }
        Ok(store)
    }

    // ── Validation de schéma ───────────────────────────────────────────────────

    /// Valide que toutes les entrées obligatoires (`required_keys`) sont présentes.
    pub fn validate_schema(&self, required_keys: &[&[u8]]) -> ExofsResult<()> {
        for &key in required_keys.iter() {
            if self.get(key).is_none() {
                return Err(ExofsError::NotFound);
            }
        }
        Ok(())
    }

    /// Valide la cohérence interne (clés uniques, longueurs).
    pub fn validate(&self) -> ExofsResult<()> {
        for (i, a) in self.entries.iter().enumerate() {
            if a.is_deleted() {
                continue;
            }
            if a.key_len == 0 || a.key_len as usize > CONFIG_KEY_LEN {
                return Err(ExofsError::Corrupt);
            }
            if a.val_len as usize > CONFIG_VALUE_LEN {
                return Err(ExofsError::Corrupt);
            }
            // Vérification de doublon (RECUR-01 : boucle imbriquée, pas récursif).
            for (j, b) in self.entries.iter().enumerate() {
                if i != j && !b.is_deleted() && a.key_matches(b.key_bytes()) {
                    return Err(ExofsError::Corrupt);
                }
            }
        }
        Ok(())
    }
}

impl fmt::Display for ConfigStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ConfigStore {{ object: {:02x?}, entries: {}, version: {}, epoch: {} }}",
            &self.object_id.0[..4],
            self.len(),
            self.version,
            self.epoch_modify.0,
        )
    }
}

impl fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── ConfigStats ────────────────────────────────────────────────────────────────

/// Statistiques des objets Config.
#[derive(Default, Debug)]
pub struct ConfigStats {
    pub total_stores: u64,
    pub total_entries: u64,
    pub tombstone_count: u64,
    pub secret_entries: u64,
    pub readonly_entries: u64,
}

impl ConfigStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, store: &ConfigStore) {
        self.total_stores = self.total_stores.saturating_add(1);
        for e in store.entries.iter() {
            self.total_entries = self.total_entries.saturating_add(1);
            if e.is_deleted() {
                self.tombstone_count = self.tombstone_count.saturating_add(1);
            }
            if e.flags & CONFIG_ENTRY_FLAG_SECRET != 0 {
                self.secret_entries = self.secret_entries.saturating_add(1);
            }
            if e.flags & CONFIG_ENTRY_FLAG_READONLY != 0 {
                self.readonly_entries = self.readonly_entries.saturating_add(1);
            }
        }
    }
}

impl fmt::Display for ConfigStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ConfigStats {{ stores: {}, entries: {}, tombstones: {}, \
             secrets: {}, readonly: {} }}",
            self.total_stores,
            self.total_entries,
            self.tombstone_count,
            self.secret_entries,
            self.readonly_entries,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> ConfigStore {
        ConfigStore::new(ObjectId([1u8; 32]), EpochId(1))
    }

    #[test]
    fn test_set_get() {
        let mut s = make_store();
        s.set(b"key1", b"value1", EpochId(2)).unwrap();
        assert_eq!(s.get(b"key1"), Some(b"value1".as_ref()));
        assert_eq!(s.get(b"key2"), None);
    }

    #[test]
    fn test_remove() {
        let mut s = make_store();
        s.set(b"x", b"y", EpochId(1)).unwrap();
        s.remove(b"x", EpochId(2)).unwrap();
        assert_eq!(s.get(b"x"), None);
    }

    #[test]
    fn test_config_entry_disk_size() {
        assert_eq!(mem::size_of::<ConfigEntryDisk>(), 360);
    }

    #[test]
    fn test_entry_roundtrip() {
        let e = ConfigEntry::from_slices(b"hostname", b"exo-os").unwrap();
        let d = e.to_disk();
        d.verify().unwrap();
        let back = ConfigEntry::from_disk(&d).unwrap();
        assert_eq!(back.key_bytes(), b"hostname");
        assert_eq!(back.value_bytes(), b"exo-os");
    }

    #[test]
    fn test_validate_schema_missing_key() {
        let s = make_store();
        assert!(s.validate_schema(&[b"required"]).is_err());
    }

    #[test]
    fn test_version_increments() {
        let mut s = make_store();
        s.set(b"k", b"v", EpochId(1)).unwrap();
        assert_eq!(s.version, 1);
        s.set(b"k", b"v2", EpochId(2)).unwrap();
        assert_eq!(s.version, 2);
    }
}
