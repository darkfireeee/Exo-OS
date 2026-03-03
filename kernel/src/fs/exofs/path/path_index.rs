//! path_index.rs — Index de répertoire on-disk ExoFS (B-tree à plat trié par hash).
//!
//! Le [`PathIndex`] est la structure qui mappe les noms d entrées d un répertoire
//! vers leurs [`ObjectId`]. Chaque répertoire ExoFS possède exactement un PathIndex
//! stocké comme payload dans son objet.
//!
//! ## Format on-disk
//! ```text
//! [PathIndexHeader : 148 bytes]
//! [PathIndexEntry  :  44 bytes] × entry_count
//! [name_len bytes de nom pour chaque entrée, sans séparateur]
//! ```
//!
//! ## Règles spec appliquées
//! - **HDR-03** : magic vérifié EN PREMIER avant tout autre champ.
//! - **ARITH-02** : `checked_add` / `checked_mul` sur tous les offsets.
//! - **OOM-02** : `try_reserve(1)` avant chaque `Vec::push`.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use core::mem::size_of;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use super::path_component::{PathComponent, validate_component, fnv1a_hash, siphash_keyed, NAME_MAX};
use super::path_index_tree::PathIndexTree;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Magic on-disk du PathIndex : `b"PIDX"`.
pub const PATH_INDEX_MAGIC: u32 = 0x50494458;
/// Version actuelle du format.
pub const PATH_INDEX_VERSION: u16 = 1;
/// Seuil de split : si entry_count dépasse cette valeur, split recommandé.
pub const PATH_INDEX_SPLIT_THRESHOLD: u32 = 192;
/// Seuil de merge : si entry_count est inférieur, merge envisageable.
pub const PATH_INDEX_MERGE_THRESHOLD: u32 = 48;

// ── PathIndexHeader ────────────────────────────────────────────────────────────

/// En-tête on-disk d un PathIndex (148 octets, `repr(C, packed)`).
///
/// **HDR-03** : `magic` est le premier champ et DOIT être vérifié avant tout accès.
/// **ONDISK-03** : pas d `AtomicU64` ici.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PathIndexHeader {
    /// Magic `PATH_INDEX_MAGIC` — vérifié EN PREMIER (HDR-03).
    pub magic:           u32,
    /// Version du format (actuellement 1).
    pub version:         u16,
    /// Flags réservés.
    pub flags:           u16,
    /// OID du répertoire parent (zéro pour la racine).
    pub parent_oid:      [u8; 32],
    /// Nombre d entrées dans cet index.
    pub entry_count:     u32,
    /// OID de l index enfant "bas" après split (zéro si pas splitté).
    pub split_low_oid:   [u8; 32],
    /// OID de l index enfant "haut" après split (zéro si pas splitté).
    pub split_high_oid:  [u8; 32],
    /// Seuil de split configuré.
    pub split_threshold: u32,
    /// Padding d alignement.
    pub _pad:            [u8; 4],
    /// Checksum Blake3 des 116 octets précédents.
    pub checksum:        [u8; 32],
}

const _: () = assert!(size_of::<PathIndexHeader>() == 148);

impl PathIndexHeader {
    /// Crée un en-tête initialisé pour un nouveau PathIndex.
    pub fn new(parent_oid: [u8; 32]) -> Self {
        PathIndexHeader {
            magic:           PATH_INDEX_MAGIC,
            version:         PATH_INDEX_VERSION,
            flags:           0,
            parent_oid,
            entry_count:     0,
            split_low_oid:   [0u8; 32],
            split_high_oid:  [0u8; 32],
            split_threshold: PATH_INDEX_SPLIT_THRESHOLD,
            _pad:            [0u8; 4],
            checksum:        [0u8; 32],
        }
    }

    /// Vérifie le magic (HDR-03).
    #[inline]
    pub fn check_magic(&self) -> ExofsResult<()> {
        // SAFETY : packed struct — accès via copy pour eviter UB sur références.
        let magic = self.magic;
        if magic != PATH_INDEX_MAGIC {
            Err(ExofsError::InvalidMagic)
        } else {
            Ok(())
        }
    }

    /// Vérifie magic + version.
    pub fn validate(&self) -> ExofsResult<()> {
        self.check_magic()?;
        if self.version != PATH_INDEX_VERSION {
            return Err(ExofsError::CorruptedStructure);
        }
        Ok(())
    }
}

// ── PathIndexEntry ─────────────────────────────────────────────────────────────

/// Entrée on-disk dans un PathIndex (44 octets, `repr(C, packed)`).
/// Suivie immédiatement de `name_len` octets de nom.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct PathIndexEntry {
    /// Hash FNV-1a du nom.
    pub hash:      u64,
    /// OID de l objet nommé par cette entrée.
    pub object_id: [u8; 32],
    /// Longueur du nom en octets.
    pub name_len:  u16,
    /// Kind (0 = Dir, 1 = File, 2 = Symlink, voir ObjectKind).
    pub kind:      u8,
    /// Padding.
    pub _pad:      u8,
}

const _: () = assert!(size_of::<PathIndexEntry>() == 44);

impl PathIndexEntry {
    /// Taille totale on-disk = header 44 + name_len octets.
    #[inline]
    pub fn on_disk_size(&self) -> usize {
        // ARITH-02
        (size_of::<PathIndexEntry>())
            .checked_add(self.name_len as usize)
            .unwrap_or(usize::MAX)
    }
}

// ── InMemoryEntry ─────────────────────────────────────────────────────────────

/// Représentation en-mémoire d une entrée PathIndex.
#[derive(Clone, Debug)]
pub struct InMemoryEntry {
    pub hash:   u64,
    pub oid:    ObjectId,
    pub kind:   u8,
    pub name:   [u8; NAME_MAX + 1],
    pub name_len: u16,
}

impl InMemoryEntry {
    pub fn name_bytes(&self) -> &[u8] { &self.name[..self.name_len as usize] }
}

// ── PathIndex ─────────────────────────────────────────────────────────────────

/// Index de répertoire en mémoire, désérialisé depuis le on-disk.
///
/// Contient une [`PathIndexTree`] pour les lookups rapides et un `Vec<InMemoryEntry>`
/// pour la sérialisation ordonnée.
pub struct PathIndex {
    /// OID du répertoire parent.
    pub parent_oid: ObjectId,
    /// Hash-table en mémoire pour lookups O(1).
    tree:         PathIndexTree,
    /// Entrées triées par hash pour la sérialisation.
    entries:      Vec<InMemoryEntry>,
    /// Dirty flag : `true` si des modifications non sérialisées existent.
    pub dirty:    bool,
    /// Split threshold configuré.
    pub split_threshold: u32,
    /// Clé secrète SipHash-2-4 (PATH-01 / S-12).
    /// Générée depuis security::crypto::rng au montage. Jamais [0u8;16] en production.
    mount_key:    [u8; 16],
}

impl PathIndex {
    /// Crée un index vide avec clé SipHash fournie au montage (PATH-01 / S-12).
    pub fn new_with_key(parent_oid: ObjectId, mount_key: [u8; 16]) -> Self {
        PathIndex {
            parent_oid,
            tree:            PathIndexTree::new_with_key(mount_key),
            entries:         Vec::new(),
            dirty:           false,
            split_threshold: PATH_INDEX_SPLIT_THRESHOLD,
            mount_key,
        }
    }

    /// Crée un index vide avec clé nulle (⚠️ tests uniquement — PATH-02).
    pub fn new(parent_oid: ObjectId) -> Self {
        Self::new_with_key(parent_oid, [0u8; 16])
    }

    /// Nombre d entrées dans l index.
    #[inline] pub fn len(&self) -> usize { self.tree.len() }
    /// `true` si aucune entrée.
    #[inline] pub fn is_empty(&self) -> bool { self.tree.is_empty() }

    /// Définit la clé SipHash de montage et reconstruit la table de hachage (PATH-01).
    ///
    /// Appeler après `from_bytes()` pour activer la protection HashDoS.
    /// Reconstruit la tree avec la nouvelle clé — O(n).
    pub fn set_mount_key(&mut self, key: [u8; 16]) -> ExofsResult<()> {
        self.mount_key = key;
        self.tree = PathIndexTree::new_with_key(key);
        let mut i = 0usize;
        while i < self.entries.len() {
            let e   = &self.entries[i];
            let name_slice = &e.name[..e.name_len as usize];
            let comp = super::path_component::validate_component(name_slice)
                .map_err(|_| ExofsError::CorruptedStructure)?;
            self.tree.insert(&comp, e.oid.clone(), e.kind)
                .map_err(|_| ExofsError::CorruptedStructure)?;
            i = i.wrapping_add(1);
        }
        Ok(())
    }

    // ── Désérialisation ───────────────────────────────────────────────────────

    /// Désérialise un PathIndex depuis des octets bruts.
    ///
    /// **HDR-03** : magic vérifié en premier.
    ///
    /// # Errors
    /// - [`ExofsError::InvalidMagic`]       si magic incorrect.
    /// - [`ExofsError::CorruptedStructure`] si trop court / incohérent.
    /// - [`ExofsError::NoMemory`]           si allocation impossible.
    pub fn from_bytes(data: &[u8]) -> ExofsResult<Self> {
        // ── 1. Vérification de taille minimale ────────────────────────────────
        if data.len() < size_of::<PathIndexHeader>() {
            return Err(ExofsError::CorruptedStructure);
        }

        // ── 2. Lecture de l en-tête (copie pour éviter UB sur packed) ─────────
        let hdr = read_header(data)?;

        // ── 3. HDR-03 : magic EN PREMIER ─────────────────────────────────────
        hdr.check_magic()?;
        hdr.validate()?;

        let entry_count = hdr.entry_count as usize;
        let parent_oid  = ObjectId(hdr.parent_oid);
        let split_thr   = hdr.split_threshold;

        // ── 4. Lecture des entrées ────────────────────────────────────────────
        let mut entries: Vec<InMemoryEntry> = Vec::new();
        let mut tree = PathIndexTree::new_with_key([0u8; 16]); // revu par set_mount_key()

        let mut offset = size_of::<PathIndexHeader>();
        for i in 0..entry_count {
            let entry_end = offset
                .checked_add(size_of::<PathIndexEntry>())
                .ok_or(ExofsError::OffsetOverflow)?;
            if entry_end > data.len() {
                return Err(ExofsError::CorruptedStructure);
            }
            let raw_entry = read_entry(&data[offset..entry_end])?;
            let name_end = entry_end
                .checked_add(raw_entry.name_len as usize)
                .ok_or(ExofsError::OffsetOverflow)?;
            if name_end > data.len() {
                return Err(ExofsError::CorruptedStructure);
            }
            let name_bytes = &data[entry_end..name_end];
            let comp = validate_component(name_bytes)
                .map_err(|_| ExofsError::CorruptedStructure)?;

            let oid = ObjectId(raw_entry.object_id);

            // Insérer dans la tree.
            tree.insert(&comp, oid.clone(), raw_entry.kind)
                .map_err(|_| ExofsError::CorruptedStructure)?;

            // Construire InMemoryEntry.
            let mut name_arr = [0u8; NAME_MAX + 1];
            name_arr[..name_bytes.len()].copy_from_slice(name_bytes);
            let ime = InMemoryEntry {
                hash:     raw_entry.hash,
                oid,
                kind:     raw_entry.kind,
                name:     name_arr,
                name_len: raw_entry.name_len,
            };
            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(ime);

            offset = name_end;
            let _ = i; // suppress unused warning
        }

        // ── 5. Vérifier cohérence entry_count ────────────────────────────────
        if entries.len() != entry_count {
            return Err(ExofsError::CorruptedStructure);
        }

        Ok(PathIndex {
            parent_oid,
            tree,
            entries,
            dirty: false,
            split_threshold: split_thr,
            mount_key: [0u8; 16], // clé initialisée à zéro lors du from_bytes
            // ⚠️ Appeler set_mount_key() après from_bytes() pour activer PATH-01.
        })
    }

    // ── Sérialisation ─────────────────────────────────────────────────────────

    /// Sérialise l index en octets pour écriture on-disk.
    ///
    /// Tri les entrées par hash avant sérialisation pour garantir l ordre.
    ///
    /// # OOM-02 / ARITH-02
    pub fn serialize(&self) -> ExofsResult<Vec<u8>> {
        // Calculer la taille totale.
        let mut total = size_of::<PathIndexHeader>();
        for e in &self.entries {
            total = total
                .checked_add(size_of::<PathIndexEntry>())
                .ok_or(ExofsError::OffsetOverflow)?
                .checked_add(e.name_len as usize)
                .ok_or(ExofsError::OffsetOverflow)?;
        }

        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;

        // Écrire l en-tête.
        let hdr = self.make_header();
        buf.extend_from_slice(bytes_of_header(&hdr));

        // Trier par hash avant écriture.
        let mut sorted: Vec<&InMemoryEntry> = self.entries.iter().collect();
        sorted.sort_unstable_by_key(|e| e.hash);

        for e in sorted {
            let raw = PathIndexEntry {
                hash:      e.hash,
                object_id: e.oid.0,
                name_len:  e.name_len,
                kind:      e.kind,
                _pad:      0,
            };
            buf.extend_from_slice(bytes_of_entry(&raw));
            buf.extend_from_slice(e.name_bytes());
        }

        Ok(buf)
    }

    // ── Opérations ────────────────────────────────────────────────────────────

    /// Recherche une entrée par composant.
    pub fn lookup(&self, comp: &PathComponent) -> Option<(ObjectId, u8)> {
        self.tree.find(comp).map(|e| (e.oid.clone(), e.kind))
    }

    /// Insère une nouvelle entrée.
    ///
    /// # Errors
    /// - [`ExofsError::ObjectAlreadyExists`] si le nom existe.
    /// - [`ExofsError::NoSpace`] si l index est plein.
    /// - [`ExofsError::NoMemory`] si allocation impossible.
    pub fn insert(&mut self, comp: &PathComponent, oid: ObjectId, kind: u8) -> ExofsResult<()> {
        self.tree.insert(comp, oid.clone(), kind)?;
        let hash = siphash_keyed(&self.mount_key, comp.as_bytes());
        let mut name_arr = [0u8; NAME_MAX + 1];
        name_arr[..comp.len()].copy_from_slice(comp.as_bytes());
        let ime = InMemoryEntry {
            hash,
            oid,
            kind,
            name: name_arr,
            name_len: comp.len() as u16,
        };
        self.entries.try_reserve(1).map_err(|_| {
            // Rollback tree.
            let _ = self.tree.remove(comp);
            ExofsError::NoMemory
        })?;
        self.entries.push(ime);
        self.dirty = true;
        Ok(())
    }

    /// Supprime une entrée.
    ///
    /// # Errors
    /// - [`ExofsError::ObjectNotFound`] si le nom n existe pas.
    pub fn remove(&mut self, comp: &PathComponent) -> ExofsResult<()> {
        self.tree.remove(comp)?;
        let name = comp.as_bytes();
        if let Some(pos) = self.entries.iter().position(|e| e.name_bytes() == name) {
            self.entries.swap_remove(pos);
        }
        self.dirty = true;
        Ok(())
    }

    /// Met à jour l OID d une entrée existante.
    pub fn update(&mut self, comp: &PathComponent, new_oid: ObjectId) -> ExofsResult<()> {
        self.tree.update_oid(comp, new_oid.clone())?;
        let name = comp.as_bytes();
        for e in &mut self.entries {
            if e.name_bytes() == name {
                e.oid = new_oid;
                break;
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// `true` si l index doit être splitté.
    #[inline]
    pub fn needs_split(&self) -> bool {
        self.entries.len() as u32 >= self.split_threshold
    }

    /// `true` si l index est éligible à un merge.
    #[inline]
    pub fn needs_merge(&self) -> bool {
        (self.entries.len() as u32) < PATH_INDEX_MERGE_THRESHOLD
    }

    /// Retourne la référence aux entrées en mémoire (pour split/merge).
    pub fn entries(&self) -> &[InMemoryEntry] { &self.entries }

    /// Retourne une référence mutable aux entrées (pour split/merge).
    pub fn entries_mut(&mut self) -> &mut Vec<InMemoryEntry> { &mut self.entries }

    /// Retourne une référence à la tree (pour inspection).
    pub fn tree(&self) -> &PathIndexTree { &self.tree }

    // ── Helpers privés ────────────────────────────────────────────────────────

    fn make_header(&self) -> PathIndexHeader {
        let mut hdr = PathIndexHeader::new(self.parent_oid.0);
        hdr.entry_count     = self.entries.len() as u32;
        hdr.split_threshold = self.split_threshold;
        hdr
    }
}

// ── Helpers de sérialisation (sans unsafe) ────────────────────────────────────

fn read_header(data: &[u8]) -> ExofsResult<PathIndexHeader> {
    if data.len() < size_of::<PathIndexHeader>() {
        return Err(ExofsError::CorruptedStructure);
    }
    let mut hdr = PathIndexHeader::new([0u8; 32]);
    // Copie octet par octet (no unsafe transmute).
    let src = &data[..size_of::<PathIndexHeader>()];
    let magic = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
    hdr.magic = magic;
    hdr.version = u16::from_le_bytes([src[4], src[5]]);
    hdr.flags   = u16::from_le_bytes([src[6], src[7]]);
    hdr.parent_oid.copy_from_slice(&src[8..40]);
    hdr.entry_count = u32::from_le_bytes([src[40], src[41], src[42], src[43]]);
    hdr.split_low_oid.copy_from_slice(&src[44..76]);
    hdr.split_high_oid.copy_from_slice(&src[76..108]);
    hdr.split_threshold = u32::from_le_bytes([src[108], src[109], src[110], src[111]]);
    // _pad [112..116], checksum [116..148]
    hdr.checksum.copy_from_slice(&src[116..148]);
    Ok(hdr)
}

fn read_entry(data: &[u8]) -> ExofsResult<PathIndexEntry> {
    if data.len() < size_of::<PathIndexEntry>() {
        return Err(ExofsError::CorruptedStructure);
    }
    let hash = u64::from_le_bytes([
        data[0], data[1], data[2], data[3],
        data[4], data[5], data[6], data[7],
    ]);
    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&data[8..40]);
    let name_len = u16::from_le_bytes([data[40], data[41]]);
    let kind     = data[42];
    Ok(PathIndexEntry { hash, object_id, name_len, kind, _pad: 0 })
}

fn bytes_of_header(hdr: &PathIndexHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(148);
    out.extend_from_slice(&hdr.magic.to_le_bytes());
    out.extend_from_slice(&hdr.version.to_le_bytes());
    out.extend_from_slice(&hdr.flags.to_le_bytes());
    out.extend_from_slice(&hdr.parent_oid);
    out.extend_from_slice(&hdr.entry_count.to_le_bytes());
    out.extend_from_slice(&hdr.split_low_oid);
    out.extend_from_slice(&hdr.split_high_oid);
    out.extend_from_slice(&hdr.split_threshold.to_le_bytes());
    out.extend_from_slice(&hdr._pad);
    out.extend_from_slice(&hdr.checksum);
    out
}

fn bytes_of_entry(e: &PathIndexEntry) -> [u8; 44] {
    let mut out = [0u8; 44];
    out[0..8].copy_from_slice(&e.hash.to_le_bytes());
    out[8..40].copy_from_slice(&e.object_id);
    out[40..42].copy_from_slice(&e.name_len.to_le_bytes());
    out[42] = e.kind;
    out[43] = 0;
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::path_component::validate_component;

    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32]; a[0] = b; ObjectId(a)
    }

    #[test] fn test_new_empty() {
        let idx = PathIndex::new(fake_oid(0));
        assert!(idx.is_empty());
    }

    #[test] fn test_insert_lookup() {
        let mut idx = PathIndex::new(fake_oid(0));
        let c = validate_component(b"hello").unwrap();
        idx.insert(&c, fake_oid(1), 0).unwrap();
        assert_eq!(idx.len(), 1);
        let (oid, kind) = idx.lookup(&c).unwrap();
        assert_eq!(oid.0[0], 1);
        assert_eq!(kind, 0);
    }

    #[test] fn test_remove() {
        let mut idx = PathIndex::new(fake_oid(0));
        let c = validate_component(b"bye").unwrap();
        idx.insert(&c, fake_oid(2), 1).unwrap();
        idx.remove(&c).unwrap();
        assert!(idx.lookup(&c).is_none());
    }

    #[test] fn test_serialize_roundtrip() {
        let mut idx = PathIndex::new(fake_oid(0));
        for i in 0u8..5 {
            let name = [b'a' + i];
            idx.insert(&validate_component(&name).unwrap(), fake_oid(i), i % 2).unwrap();
        }
        let bytes = idx.serialize().unwrap();
        let idx2  = PathIndex::from_bytes(&bytes).unwrap();
        assert_eq!(idx2.len(), 5);
        for i in 0u8..5 {
            let c = validate_component(&[b'a' + i]).unwrap();
            assert!(idx2.lookup(&c).is_some());
        }
    }

    #[test] fn test_bad_magic() {
        let mut bytes = vec![0u8; 148];
        assert!(matches!(PathIndex::from_bytes(&bytes), Err(ExofsError::InvalidMagic)));
        // Mettre le bon magic.
        bytes[0..4].copy_from_slice(&PATH_INDEX_MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&(99u16).to_le_bytes()); // mauvaise version
        assert!(matches!(PathIndex::from_bytes(&bytes), Err(ExofsError::CorruptedStructure)));
    }

    #[test] fn test_needs_split() {
        let mut idx = PathIndex::new(fake_oid(0));
        idx.split_threshold = 3;
        for i in 0u8..3 {
            idx.insert(&validate_component(&[b'a' + i]).unwrap(), fake_oid(i), 0).unwrap();
        }
        assert!(idx.needs_split());
    }
}
