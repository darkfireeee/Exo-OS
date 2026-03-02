// path/path_index.rs — PathIndex : répertoire ExoFS
// Ring 0, no_std
//
// STRUCTURE ON-DISK :
//   Sorted array (hash:u64, ObjectId:32B, name_len:u16, kind:u8)
//   ─ format fixe, aligné, #[repr(C, packed)]
//
// IN-MEMORY : radix tree pour lookup O(log n)
//
// RÈGLES :
//   • SPLIT-02 : UN SEUL EpochRoot pour un split atomique
//   • MAGIC-01 : vérifier magic PATH_INDEX_MAGIC en premier

use crate::fs::exofs::core::{
    ObjectId, BlobId, EpochId, DiskOffset, ExofsError,
    PATH_INDEX_MAGIC, OBJECT_HEADER_MAGIC, ObjectKind,
};
use crate::fs::exofs::path::path_component::PathComponent;
use crate::fs::exofs::path::path_index_tree::PathIndexTree;
use crate::fs::exofs::epoch::epoch_delta::CURRENT_EPOCH_DELTA;
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::mem::size_of;

// ─── Structure on-disk ────────────────────────────────────────────────────────

/// En-tête PathIndex on-disk — MAGIC EN PREMIER (règle MAGIC-01)
#[repr(C, packed)]
pub struct PathIndexHeader {
    /// Magic = PATH_INDEX_MAGIC (0x58445049 = "PIDX")
    pub magic: u32,
    /// Version du format
    pub version: u16,
    /// Flags
    pub flags: u16,
    /// ObjectId du répertoire parent (INVALID pour la racine)
    pub parent_oid: [u8; 32],
    /// Nombre d'entrées dans ce node
    pub entry_count: u32,
    /// ObjectId du child low (si splitté)
    pub split_low_oid: [u8; 32],
    /// ObjectId du child high (si splitté)
    pub split_high_oid: [u8; 32],
    /// Seuil de split
    pub split_threshold: u32,
    /// Padding pour alignement
    pub _pad: [u8; 4],
    /// Checksum Blake3 de tout ce qui précède
    pub checksum: [u8; 32],
}

const _: () = assert!(size_of::<PathIndexHeader>() == 148);

/// Entrée PathIndex on-disk (format fixe)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PathIndexEntry {
    /// Hash FNV-1a du nom (pour tri rapide)
    pub hash: u64,
    /// ObjectId pointé
    pub object_id: [u8; 32],
    /// Longueur du nom
    pub name_len: u16,
    /// Type de l'objet
    pub kind: u8,
    /// Padding
    pub _pad: u8,
    // Suivi immédiatement par `name_len` octets de nom (variable)
}

const _: () = assert!(size_of::<PathIndexEntry>() == 44);

// ─── Structure in-memory ──────────────────────────────────────────────────────

/// PathIndex en mémoire — contient le radix tree et les entrées
pub struct PathIndex {
    /// ObjectId de ce PathIndex (= ObjectId du répertoire)
    pub self_oid: ObjectId,
    /// ObjectId du parent
    pub parent_oid: ObjectId,
    /// Arbre radix pour lookup O(log n)
    pub tree: PathIndexTree,
    /// Entrées (pour itération)
    pub entries: Vec<(u64, ObjectId, Vec<u8>)>, // (hash, oid, name)
    /// Dirty flag — modifié depuis le dernier commit
    pub dirty: bool,
    /// Epoch du dernier load/modification
    pub epoch: EpochId,
}

impl PathIndex {
    /// Charge un PathIndex depuis le disque (via object_reader)
    pub fn load(dir_oid: ObjectId, epoch: EpochId) -> Result<Self, ExofsError> {
        use crate::fs::exofs::storage::object_reader::read_object_payload;

        let payload = read_object_payload(dir_oid, epoch)?;
        if payload.len() < size_of::<PathIndexHeader>() {
            return Err(ExofsError::CorruptedStructure);
        }

        // MAGIC EN PREMIER (règle MAGIC-01)
        // SAFETY: payload est aligné et de taille suffisante
        let header = unsafe {
            &*(payload.as_ptr() as *const PathIndexHeader)
        };
        let magic = { let m = header.magic; m }; // unaligned read
        if magic != PATH_INDEX_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }

        // Checksum Blake3
        let checksum_offset = size_of::<PathIndexHeader>() - 32;
        let stored_checksum = &payload[checksum_offset..checksum_offset + 32];
        let computed = crate::fs::exofs::core::blake3_hash(&payload[..checksum_offset]);
        if stored_checksum != &computed {
            return Err(ExofsError::ChecksumMismatch);
        }

        let parent_oid = ObjectId({ let p = header.parent_oid; p });
        let entry_count = { let e = header.entry_count; e } as usize;

        let mut index = PathIndex {
            self_oid: dir_oid,
            parent_oid,
            tree: PathIndexTree::new(),
            entries: Vec::new(),
            dirty: false,
            epoch,
        };
        index.entries.try_reserve(entry_count)
            .map_err(|_| ExofsError::NoMemory)?;

        // Parse les entrées depuis le payload
        let mut offset = size_of::<PathIndexHeader>();
        for _ in 0..entry_count {
            if offset + size_of::<PathIndexEntry>() > payload.len() {
                return Err(ExofsError::CorruptedStructure);
            }
            // SAFETY: offset est dans les bornes du payload
            let entry = unsafe {
                &*(payload.as_ptr().add(offset) as *const PathIndexEntry)
            };
            let hash = { let h = entry.hash; h };
            let oid = ObjectId({ let o = entry.object_id; o });
            let name_len = { let n = entry.name_len; n } as usize;
            offset += size_of::<PathIndexEntry>();

            if offset + name_len > payload.len() {
                return Err(ExofsError::CorruptedStructure);
            }
            let name = payload[offset..offset + name_len].to_vec();
            offset = offset.checked_add(name_len)
                .ok_or(ExofsError::OffsetOverflow)?;

            index.tree.insert(hash, oid);
            index.entries.push((hash, oid, name));
        }

        Ok(index)
    }

    /// Lookup d'un nom dans ce PathIndex
    pub fn lookup(&self, name: &[u8]) -> Result<ObjectId, ExofsError> {
        let hash = fnv_hash(name);
        self.tree.find(hash, name, &self.entries)
            .ok_or(ExofsError::ObjectNotFound)
    }

    /// Retourne l'ObjectId du parent
    pub fn parent_oid(&self) -> Result<ObjectId, ExofsError> {
        if self.parent_oid == ObjectId::INVALID {
            // À la racine — retourner la racine elle-même
            Ok(self.self_oid)
        } else {
            Ok(self.parent_oid)
        }
    }

    /// Insère une nouvelle entrée (marque dirty)
    pub fn insert(
        &mut self,
        name: &[u8],
        oid: ObjectId,
        kind: ObjectKind,
    ) -> Result<(), ExofsError> {
        if name.len() > crate::fs::exofs::core::NAME_MAX {
            return Err(ExofsError::NameTooLong);
        }
        let hash = fnv_hash(name);
        self.tree.insert(hash, oid);
        let mut name_vec = Vec::new();
        name_vec.try_reserve(name.len()).map_err(|_| ExofsError::NoMemory)?;
        name_vec.extend_from_slice(name);
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push((hash, oid, name_vec));
        self.dirty = true;

        // Enregistre la modification dans le delta epoch courant
        CURRENT_EPOCH_DELTA.record_path_index_modified(self.self_oid);
        Ok(())
    }

    /// Supprime une entrée
    pub fn remove(&mut self, name: &[u8]) -> Result<(), ExofsError> {
        let hash = fnv_hash(name);
        let pos = self.entries.iter().position(|(h, _, n)| {
            *h == hash && n.as_slice() == name
        }).ok_or(ExofsError::ObjectNotFound)?;
        self.entries.remove(pos);
        self.tree.remove(hash);
        self.dirty = true;
        CURRENT_EPOCH_DELTA.record_path_index_modified(self.self_oid);
        Ok(())
    }

    /// Nombre d'entrées
    #[inline]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Vérifie si un split est nécessaire
    #[inline]
    pub fn needs_split(&self) -> bool {
        self.entries.len() >= crate::fs::exofs::core::PATH_INDEX_SPLIT_THRESHOLD
    }

    /// Vérifie si un merge est nécessaire
    #[inline]
    pub fn needs_merge(&self) -> bool {
        self.entries.len() < crate::fs::exofs::core::PATH_INDEX_MERGE_THRESHOLD
    }
}

// ─── Utilitaires ──────────────────────────────────────────────────────────────

/// Hash FNV-1a d'un nom
#[inline]
pub fn fnv_hash(name: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in name {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
