//! path_index_tree.rs — Table de hachage à adressage ouvert pour les entrées PathIndex.
//!
//! Cette structure est un composant interne utilisé par [`PathIndex`] pour stocker
//! les entrées d un répertoire de façon efficace sans allocation dynamique par entrée.
//!
//! ## Caractéristiques
//! - 256 buckets (adressage ouvert, sondage linéaire).
//! - Facteur de charge max : 75 % (192 entrées).
//! - Pas d allocation dynamique (tous les buckets sont statiquement sized).
//! - Clé = hash FNV-1a u64 + tranché de nom (pour distinguer collisions).
//! - Opérations O(1) amorti : insert, find, remove.
//!
//! # Règles spec appliquées
//! - **ARITH-02** : `checked_add` / `wrapping_add` sur tous les calculs d index.
//! - **OOM-02** : pas d allocation heap — tous les buckets sont inline.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use super::path_component::{PathComponent, NAME_MAX, siphash_keyed};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Nombre de buckets de la table (puissance de 2).
pub const TREE_BUCKETS: usize = 256;
/// Facteur de charge maximal : 75 %.
pub const TREE_MAX_LOAD: usize = (TREE_BUCKETS * 3) / 4;
/// Valeur sentinel pour un bucket vide.
const EMPTY_HASH: u64 = 0;
/// Valeur sentinel pour un bucket supprimé (tombstone).
const DELETED_HASH: u64 = u64::MAX;

// ── TreeEntry ─────────────────────────────────────────────────────────────────

/// Entrée stockée dans un bucket de la table.
#[derive(Clone)]
pub struct TreeEntry {
    /// Hash FNV-1a du nom (0 = vide, u64::MAX = supprimé).
    pub hash:   u64,
    /// Identifiant d objet associé.
    pub oid:    ObjectId,
    /// Nom du composant (stockage inline, jamais heap).
    pub name:   [u8; NAME_MAX + 1],
    pub name_len: u16,
    /// Kind brut (0 = Directory, 1 = File, 2 = Symlink, …).
    pub kind:   u8,
}

impl TreeEntry {
    /// Crée un bucket vide.
    const fn empty() -> Self {
        TreeEntry {
            hash:     EMPTY_HASH,
            oid:      ObjectId::INVALID,
            name:     [0u8; NAME_MAX + 1],
            name_len: 0,
            kind:     0,
        }
    }

    /// `true` si le slot est libre.
    #[inline] pub fn is_empty(&self)   -> bool { self.hash == EMPTY_HASH }
    /// `true` si le slot a été supprimé (tombstone).
    #[inline] pub fn is_deleted(&self) -> bool { self.hash == DELETED_HASH }
    /// `true` si le slot est occupé.
    #[inline] pub fn is_occupied(&self) -> bool {
        self.hash != EMPTY_HASH && self.hash != DELETED_HASH
    }

    /// Retourne la vue &[u8] du nom.
    #[inline]
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    /// Vérifie si cet slot correspond au composant `comp`.
    #[inline]
    pub fn matches(&self, hash: u64, name: &[u8]) -> bool {
        self.is_occupied() && self.hash == hash && self.name_bytes() == name
    }
}

impl core::fmt::Debug for TreeEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_empty() {
            write!(f, "<empty>")
        } else if self.is_deleted() {
            write!(f, "<deleted>")
        } else {
            write!(f, "TreeEntry {{ name: {:?}, kind: {} }}",
                core::str::from_utf8(self.name_bytes()).unwrap_or("?"),
                self.kind)
        }
    }
}

// ── PathIndexTree ─────────────────────────────────────────────────────────────

/// Table de hachage à adressage ouvert pour les entrées de répertoire.
///
/// Taille fixe, pas d allocation dynamique. Max [`TREE_MAX_LOAD`] entrées.
pub struct PathIndexTree {
    buckets:    [TreeEntry; TREE_BUCKETS],
    count:      usize,
    /// Nombre de tombstones actifs (aide à décider un rebuild).
    tombstones: usize,
    /// Clé secrète SipHash-2-4 (PATH-01). Générée aléatoirement au montage.
    key:        [u8; 16],
}

impl PathIndexTree {
    /// Crée une table vide avec clé aléatoire fournie au montage (PATH-01).
    pub fn new_with_key(key: [u8; 16]) -> Self {
        PathIndexTree {
            buckets:    core::array::from_fn(|_| TreeEntry::empty()),
            count:      0,
            tombstones: 0,
            key,
        }
    }

    /// Crée une table vide avec clé nulle (usage tests / pré-montage uniquement).
    /// ⚠️ PATH-02 : clé nulle = vulnérable HashDoS. Utiliser new_with_key() en production.
    pub fn new() -> Self { Self::new_with_key([0u8; 16]) }

    /// Nombre d entrées actives.
    #[inline] pub fn len(&self) -> usize { self.count }
    /// `true` si aucune entrée.
    #[inline] pub fn is_empty(&self) -> bool { self.count == 0 }
    /// `true` si la table est pleine (facteur de charge atteint).
    #[inline] pub fn is_full(&self) -> bool { self.count >= TREE_MAX_LOAD }

    /// Insère une entrée dans la table.
    ///
    /// # Errors
    /// - [`ExofsError::NoSpace`]     si la table est pleine.
    /// - [`ExofsError::PathTooLong`] si le nom dépasse `NAME_MAX`.
    /// - [`ExofsError::ObjectAlreadyExists`] si une entrée avec ce nom existe déjà.
    pub fn insert(&mut self, comp: &PathComponent, oid: ObjectId, kind: u8) -> ExofsResult<()> {
        if self.is_full() { return Err(ExofsError::NoSpace); }
        let name  = comp.as_bytes();
        let hash  = Self::safe_hash(siphash_keyed(&self.key, name));
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut first_tomb: Option<usize> = None;
        let mut idx = start;

        for _ in 0..TREE_BUCKETS {
            let b = &self.buckets[idx];
            if b.is_empty() {
                // Insérer ici (ou au tombstone si trouvé plus tôt).
                let dest = first_tomb.unwrap_or(idx);
                self.write_entry(dest, hash, oid, name, kind);
                self.count = self.count.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
                if first_tomb.is_some() {
                    self.tombstones = self.tombstones.saturating_sub(1);
                }
                return Ok(());
            }
            if b.is_deleted() {
                if first_tomb.is_none() { first_tomb = Some(idx); }
            } else if b.matches(hash, name) {
                return Err(ExofsError::ObjectAlreadyExists);
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        Err(ExofsError::NoSpace)
    }

    /// Recherche une entrée par composant.
    ///
    /// Retourne une référence vers l entrée si trouvée.
    pub fn find(&self, comp: &PathComponent) -> Option<&TreeEntry> {
        let name = comp.as_bytes();
        let hash = Self::safe_hash(siphash_keyed(&self.key, name));
        self.find_by_hash_name(hash, name)
    }

    /// Recherche par hash et octets de nom.
    pub fn find_by_hash_name(&self, hash: u64, name: &[u8]) -> Option<&TreeEntry> {
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut idx = start;
        for _ in 0..TREE_BUCKETS {
            let b = &self.buckets[idx];
            if b.is_empty() { return None; }
            if b.matches(hash, name) { return Some(b); }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        None
    }

    /// Supprime une entrée (pose un tombstone).
    ///
    /// # Errors
    /// - [`ExofsError::ObjectNotFound`] si l entrée n existe pas.
    pub fn remove(&mut self, comp: &PathComponent) -> ExofsResult<()> {
        let name = comp.as_bytes();
        let hash = Self::safe_hash(siphash_keyed(&self.key, name));
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut idx = start;
        for _ in 0..TREE_BUCKETS {
            let b = &self.buckets[idx];
            if b.is_empty() { return Err(ExofsError::ObjectNotFound); }
            if b.matches(hash, name) {
                self.buckets[idx].hash = DELETED_HASH;
                self.count = self.count.saturating_sub(1);
                self.tombstones = self.tombstones.checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
                return Ok(());
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        Err(ExofsError::ObjectNotFound)
    }

    /// Met à jour l ObjectId d une entrée existante.
    ///
    /// # Errors
    /// - [`ExofsError::ObjectNotFound`] si l entrée n existe pas.
    pub fn update_oid(&mut self, comp: &PathComponent, new_oid: ObjectId) -> ExofsResult<()> {
        let name = comp.as_bytes();
        let hash = Self::safe_hash(comp.fnv1a());
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut idx = start;
        for _ in 0..TREE_BUCKETS {
            let b = &mut self.buckets[idx];
            if b.is_empty() { return Err(ExofsError::ObjectNotFound); }
            if b.is_occupied() && b.hash == hash && b.name_bytes() == name {
                b.oid = new_oid;
                return Ok(());
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        Err(ExofsError::ObjectNotFound)
    }

    /// Retourne un itérateur sur les entrées occupées.
    pub fn iter(&self) -> TreeIter<'_> {
        TreeIter { tree: self, idx: 0 }
    }

    /// Collecte toutes les entrées dans un Vec trié par hash.
    ///
    /// # OOM-02
    pub fn sorted_entries(&self) -> ExofsResult<Vec<TreeEntryRef<'_>>> {
        let mut out: Vec<TreeEntryRef<'_>> = Vec::new();
        for b in &self.buckets {
            if b.is_occupied() {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(TreeEntryRef { entry: b });
            }
        }
        out.sort_unstable_by_key(|e| e.entry.hash);
        Ok(out)
    }

    /// Reconstruit la table sans tombstones (compacte).
    ///
    /// Crée une nouvelle table et réinsère toutes les entrées valides.
    pub fn compact(&mut self) -> ExofsResult<()> {
        let mut fresh = PathIndexTree::new();
        for b in &self.buckets {
            if b.is_occupied() {
                let comp = PathComponent::from_validated(b.name_bytes());
                fresh.insert(&comp, b.oid.clone(), b.kind)?;
            }
        }
        *self = fresh;
        Ok(())
    }

    /// `true` si un compactage est conseillé (>30 % de tombstones).
    pub fn should_compact(&self) -> bool {
        self.tombstones > TREE_BUCKETS / 3
    }

    /// Vide la table.
    pub fn clear(&mut self) {
        for b in &mut self.buckets { *b = TreeEntry::empty(); }
        self.count = 0;
        self.tombstones = 0;
    }

    /// Charge les entrées depuis un slice ordonné (par hash) — utilisé par PathIndex::from_bytes.
    ///
    /// Les entrées doivent déjà être validées (magic + checksum vérifiés par l appelant).
    pub fn load_sorted(&mut self, entries: &[(u64, ObjectId, &[u8], u8)]) -> ExofsResult<()> {
        for &(hash, ref oid, name, kind) in entries {
            let safe_hash = Self::safe_hash(hash);
            let start = (safe_hash as usize) & (TREE_BUCKETS - 1);
            let mut idx = start;
            let mut inserted = false;
            for _ in 0..TREE_BUCKETS {
                let b = &self.buckets[idx];
                if b.is_empty() || b.is_deleted() {
                    self.write_entry(idx, safe_hash, oid.clone(), name, kind);
                    self.count = self.count.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
                    inserted = true;
                    break;
                }
                idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
            }
            if !inserted { return Err(ExofsError::NoSpace); }
        }
        Ok(())
    }

    // ── Helpers privés ────────────────────────────────────────────────────────

    /// Transforme le hash brut en hash safe (évite 0 et u64::MAX réservés).
    #[inline]
    fn safe_hash(h: u64) -> u64 {
        if h == EMPTY_HASH || h == DELETED_HASH { h.wrapping_add(1) } else { h }
    }

    fn write_entry(&mut self, idx: usize, hash: u64, oid: ObjectId, name: &[u8], kind: u8) {
        let b = &mut self.buckets[idx];
        b.hash = hash;
        b.oid  = oid;
        b.name_len = name.len() as u16;
        b.name[..name.len()].copy_from_slice(name);
        b.kind = kind;
    }
}

impl Default for PathIndexTree {
    fn default() -> Self { Self::new() }
}

// ── Itérateur ─────────────────────────────────────────────────────────────────

/// Itérateur sur les entrées occupées de [`PathIndexTree`].
pub struct TreeIter<'a> {
    tree: &'a PathIndexTree,
    idx:  usize,
}

impl<'a> Iterator for TreeIter<'a> {
    type Item = &'a TreeEntry;
    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < TREE_BUCKETS {
            let b = &self.tree.buckets[self.idx];
            self.idx = self.idx.wrapping_add(1);
            if b.is_occupied() { return Some(b); }
        }
        None
    }
}

/// Wrapper de référence pour la collection triée.
pub struct TreeEntryRef<'a> {
    pub entry: &'a TreeEntry,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::path_component::validate_component;

    fn fake_oid(b: u8) -> ObjectId {
        let mut arr = [0u8; 32];
        arr[0] = b;
        ObjectId(arr)
    }

    #[test] fn test_insert_find() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"hello").unwrap();
        t.insert(&c, fake_oid(1), 0).unwrap();
        let e = t.find(&c).unwrap();
        assert_eq!(e.name_bytes(), b"hello");
        assert_eq!(e.oid.0[0], 1);
    }
    #[test] fn test_duplicate_insert() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"dup").unwrap();
        t.insert(&c, fake_oid(1), 0).unwrap();
        assert!(matches!(t.insert(&c, fake_oid(2), 0), Err(ExofsError::ObjectAlreadyExists)));
    }
    #[test] fn test_remove() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"bye").unwrap();
        t.insert(&c, fake_oid(1), 0).unwrap();
        t.remove(&c).unwrap();
        assert!(t.find(&c).is_none());
        assert_eq!(t.len(), 0);
    }
    #[test] fn test_remove_not_found() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"x").unwrap();
        assert!(matches!(t.remove(&c), Err(ExofsError::ObjectNotFound)));
    }
    #[test] fn test_full() {
        let mut t = PathIndexTree::new();
        for i in 0u8..=191 {
            let name = [b'a' + (i % 26), b'0' + (i / 26)];
            let c = validate_component(&name).unwrap();
            t.insert(&c, fake_oid(i), 0).unwrap();
        }
        let extra = validate_component(b"extra").unwrap();
        assert!(matches!(t.insert(&extra, fake_oid(0), 0), Err(ExofsError::NoSpace)));
    }
    #[test] fn test_compact() {
        let mut t = PathIndexTree::new();
        for i in 0u8..10 {
            let name = [b'a' + i];
            let c = validate_component(&name).unwrap();
            t.insert(&c, fake_oid(i), 0).unwrap();
        }
        let c = validate_component(b"a").unwrap();
        t.remove(&c).unwrap();
        t.compact().unwrap();
        assert_eq!(t.len(), 9);
        assert_eq!(t.tombstones, 0);
    }
    #[test] fn test_iter_count() {
        let mut t = PathIndexTree::new();
        for i in 0u8..5 {
            let name = [b'x' + i];
            let c = validate_component(&name).unwrap();
            t.insert(&c, fake_oid(i), 0).unwrap();
        }
        assert_eq!(t.iter().count(), 5);
    }
    #[test] fn test_update_oid() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"upd").unwrap();
        t.insert(&c, fake_oid(1), 0).unwrap();
        t.update_oid(&c, fake_oid(99)).unwrap();
        assert_eq!(t.find(&c).unwrap().oid.0[0], 99);
    }
}
