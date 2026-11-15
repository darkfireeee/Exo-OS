//! path_index_tree.rs — Open-addressing hash table for PathIndex entries.
//!
//! This structure is an internal component used by [`PathIndex`] to store
//! directory entries efficiently without per-entry dynamic allocation.
//!
//! ## Characteristics
//! - 256 buckets (open addressing, linear probing).
//! - Max load factor: 75% (192 entries).
//! - Buckets lazily allocated on the heap to preserve kernel stacks.
//! - Key = u64 SipHash/FNV-1a hash + name slice (to resolve collisions).
//! - Amortized O(1) operations: insert, find, remove.
//!
//! # Applied spec rules
//! - **ARITH-02** : `checked_add` / `wrapping_add` on all index calculations.
//! - **OOM-02** : fallible allocation before initializing buckets.

extern crate alloc;
use alloc::vec::Vec;

use super::path_component::{siphash_keyed, PathComponent, NAME_MAX};
use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};

// ── Constants ────────────────────────────────────────────────────────────────

/// Number of table buckets (power of 2).
pub const TREE_BUCKETS: usize = 256;
/// Maximum load factor: 75%.
pub const TREE_MAX_LOAD: usize = (TREE_BUCKETS * 3) / 4;
/// Sentinel value for an empty bucket.
const EMPTY_HASH: u64 = 0;
/// Sentinel value for a deleted bucket (tombstone).
const DELETED_HASH: u64 = u64::MAX;

// ── TreeEntry ─────────────────────────────────────────────────────────────────

/// Entry stored in a table bucket.
#[derive(Clone)]
pub struct TreeEntry {
    /// FNV-1a hash of name (0 = empty, u64::MAX = deleted).
    pub hash: u64,
    /// Associated object identifier.
    pub oid: ObjectId,
    /// Component name (inline storage, never heap).
    pub name: [u8; NAME_MAX + 1],
    pub name_len: u16,
    /// Raw kind (0 = Directory, 1 = File, 2 = Symlink, …).
    pub kind: u8,
}

impl TreeEntry {
    /// Creates an empty bucket.
    const fn empty() -> Self {
        TreeEntry {
            hash: EMPTY_HASH,
            oid: ObjectId::INVALID,
            name: [0u8; NAME_MAX + 1],
            name_len: 0,
            kind: 0,
        }
    }

    /// `true` if slot is free.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.hash == EMPTY_HASH
    }
    /// `true` if slot was deleted (tombstone).
    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.hash == DELETED_HASH
    }
    /// `true` if slot is occupied.
    #[inline]
    pub fn is_occupied(&self) -> bool {
        self.hash != EMPTY_HASH && self.hash != DELETED_HASH
    }

    /// Returns &[u8] view of the name.
    #[inline]
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    /// Checks if this slot matches the component `comp`.
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
            write!(
                f,
                "TreeEntry {{ name: {:?}, kind: {} }}",
                core::str::from_utf8(self.name_bytes()).unwrap_or("?"),
                self.kind
            )
        }
    }
}

// ── PathIndexTree ─────────────────────────────────────────────────────────────

/// Open-addressing hash table for directory entries.
///
/// Fixed logical size, with buckets lazily allocated off kernel stack.
/// Max [`TREE_MAX_LOAD`] entries.
pub struct PathIndexTree {
    buckets: Vec<TreeEntry>,
    count: usize,
    /// Number of active tombstones (helps decide when to rebuild).
    tombstones: usize,
    /// SipHash-2-4 secret key (PATH-01). Generated randomly at mount time.
    key: [u8; 16],
}

impl PathIndexTree {
    /// Creates an empty table with random key provided at mount (PATH-01).
    pub fn new_with_key(key: [u8; 16]) -> Self {
        PathIndexTree {
            buckets: Vec::new(),
            count: 0,
            tombstones: 0,
            key,
        }
    }

    /// Creates an empty table with null key (test / pre-mount usage only).
    /// WARNING PATH-02: null key = vulnerable to HashDoS. Use new_with_key() in production.
    pub fn new() -> Self {
        Self::new_with_key([0u8; 16])
    }

    /// Number of active entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }
    /// `true` if no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    /// `true` if table is full (load factor reached).
    #[inline]
    pub fn is_full(&self) -> bool {
        self.count >= TREE_MAX_LOAD
    }

    /// Inserts an entry into the table.
    ///
    /// # Errors
    /// - [`ExofsError::NoSpace`]     if table is full.
    /// - [`ExofsError::PathTooLong`] if name exceeds `NAME_MAX`.
    /// - [`ExofsError::ObjectAlreadyExists`] if an entry with this name already exists.
    pub fn insert(&mut self, comp: &PathComponent, oid: ObjectId, kind: u8) -> ExofsResult<()> {
        self.ensure_buckets()?;
        if self.is_full() {
            return Err(ExofsError::NoSpace);
        }
        let name = comp.as_bytes();
        let hash = Self::safe_hash(siphash_keyed(&self.key, name));
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut first_tomb: Option<usize> = None;
        let mut idx = start;

        for _ in 0..TREE_BUCKETS {
            let b = &self.buckets[idx];
            if b.is_empty() {
                // Insert here (or at tombstone if found earlier).
                let dest = first_tomb.unwrap_or(idx);
                self.write_entry(dest, hash, oid, name, kind);
                self.count = self
                    .count
                    .checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
                if first_tomb.is_some() {
                    self.tombstones = self.tombstones.saturating_sub(1);
                }
                return Ok(());
            }
            if b.is_deleted() {
                if first_tomb.is_none() {
                    first_tomb = Some(idx);
                }
            } else if b.matches(hash, name) {
                return Err(ExofsError::ObjectAlreadyExists);
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        Err(ExofsError::NoSpace)
    }

    /// Looks up an entry by component.
    ///
    /// Returns a reference to the entry if found.
    pub fn find(&self, comp: &PathComponent) -> Option<&TreeEntry> {
        let name = comp.as_bytes();
        let hash = Self::safe_hash(siphash_keyed(&self.key, name));
        self.find_by_hash_name(hash, name)
    }

    /// Looks up by hash and name bytes.
    pub fn find_by_hash_name(&self, hash: u64, name: &[u8]) -> Option<&TreeEntry> {
        if self.buckets.len() != TREE_BUCKETS {
            return None;
        }
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut idx = start;
        for _ in 0..TREE_BUCKETS {
            let b = &self.buckets[idx];
            if b.is_empty() {
                return None;
            }
            if b.matches(hash, name) {
                return Some(b);
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        None
    }

    /// Removes an entry (places a tombstone).
    ///
    /// # Errors
    /// - [`ExofsError::ObjectNotFound`] if entry does not exist.
    pub fn remove(&mut self, comp: &PathComponent) -> ExofsResult<()> {
        if self.buckets.len() != TREE_BUCKETS {
            return Err(ExofsError::ObjectNotFound);
        }
        let name = comp.as_bytes();
        let hash = Self::safe_hash(siphash_keyed(&self.key, name));
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut idx = start;
        for _ in 0..TREE_BUCKETS {
            let b = &self.buckets[idx];
            if b.is_empty() {
                return Err(ExofsError::ObjectNotFound);
            }
            if b.matches(hash, name) {
                self.buckets[idx].hash = DELETED_HASH;
                self.count = self.count.saturating_sub(1);
                self.tombstones = self
                    .tombstones
                    .checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
                return Ok(());
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        Err(ExofsError::ObjectNotFound)
    }

    /// Updates the ObjectId of an existing entry.
    ///
    /// # Errors
    /// - [`ExofsError::ObjectNotFound`] if entry does not exist.
    pub fn update_oid(&mut self, comp: &PathComponent, new_oid: ObjectId) -> ExofsResult<()> {
        if self.buckets.len() != TREE_BUCKETS {
            return Err(ExofsError::ObjectNotFound);
        }
        let name = comp.as_bytes();
        let hash = Self::safe_hash(siphash_keyed(&self.key, name));
        let start = (hash as usize) & (TREE_BUCKETS - 1);
        let mut idx = start;
        for _ in 0..TREE_BUCKETS {
            let b = &mut self.buckets[idx];
            if b.is_empty() {
                return Err(ExofsError::ObjectNotFound);
            }
            if b.is_occupied() && b.hash == hash && b.name_bytes() == name {
                b.oid = new_oid;
                return Ok(());
            }
            idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
        }
        Err(ExofsError::ObjectNotFound)
    }

    /// Returns an iterator over occupied entries.
    pub fn iter(&self) -> TreeIter<'_> {
        TreeIter { tree: self, idx: 0 }
    }

    /// Collects all entries into a Vec sorted by hash.
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

    /// Rebuilds table without tombstones (compaction).
    ///
    /// Creates a fresh table and re-inserts all valid entries.
    pub fn compact(&mut self) -> ExofsResult<()> {
        let mut fresh = PathIndexTree::new_with_key(self.key);
        for b in &self.buckets {
            if b.is_occupied() {
                let comp = PathComponent::from_validated(b.name_bytes());
                fresh.insert(&comp, b.oid.clone(), b.kind)?;
            }
        }
        *self = fresh;
        Ok(())
    }

    /// `true` if compaction is recommended (> 30% tombstones).
    pub fn should_compact(&self) -> bool {
        self.tombstones > TREE_BUCKETS / 3
    }

    /// Clears the table.
    pub fn clear(&mut self) {
        for b in &mut self.buckets {
            *b = TreeEntry::empty();
        }
        self.count = 0;
        self.tombstones = 0;
    }

    /// Loads entries from an ordered slice (by hash) — used by PathIndex::from_bytes.
    ///
    /// Entries must already be validated (magic + checksum checked by caller).
    pub fn load_sorted(&mut self, entries: &[(u64, ObjectId, &[u8], u8)]) -> ExofsResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.ensure_buckets()?;
        for &(hash, ref oid, name, kind) in entries {
            let safe_hash = Self::safe_hash(hash);
            let start = (safe_hash as usize) & (TREE_BUCKETS - 1);
            let mut idx = start;
            let mut inserted = false;
            for _ in 0..TREE_BUCKETS {
                let b = &self.buckets[idx];
                if b.is_empty() || b.is_deleted() {
                    self.write_entry(idx, safe_hash, oid.clone(), name, kind);
                    self.count = self
                        .count
                        .checked_add(1)
                        .ok_or(ExofsError::OffsetOverflow)?;
                    inserted = true;
                    break;
                }
                idx = idx.wrapping_add(1) & (TREE_BUCKETS - 1);
            }
            if !inserted {
                return Err(ExofsError::NoSpace);
            }
        }
        Ok(())
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    /// Transforms raw hash into safe hash (avoids reserved 0 and u64::MAX).
    #[inline]
    fn safe_hash(h: u64) -> u64 {
        if h == EMPTY_HASH || h == DELETED_HASH {
            h.wrapping_add(1)
        } else {
            h
        }
    }

    fn ensure_buckets(&mut self) -> ExofsResult<()> {
        if self.buckets.len() == TREE_BUCKETS {
            return Ok(());
        }
        if !self.buckets.is_empty() {
            return Err(ExofsError::CorruptedStructure);
        }

        self.buckets
            .try_reserve_exact(TREE_BUCKETS)
            .map_err(|_| ExofsError::NoMemory)?;
        while self.buckets.len() < TREE_BUCKETS {
            self.buckets.push(TreeEntry::empty());
        }
        Ok(())
    }

    fn write_entry(&mut self, idx: usize, hash: u64, oid: ObjectId, name: &[u8], kind: u8) {
        let b = &mut self.buckets[idx];
        b.hash = hash;
        b.oid = oid;
        b.name_len = name.len() as u16;
        b.name[..name.len()].copy_from_slice(name);
        b.kind = kind;
    }
}

impl Default for PathIndexTree {
    fn default() -> Self {
        Self::new()
    }
}

// ── Iterator ─────────────────────────────────────────────────────────────────

/// Iterator over occupied entries of [`PathIndexTree`].
pub struct TreeIter<'a> {
    tree: &'a PathIndexTree,
    idx: usize,
}

impl<'a> Iterator for TreeIter<'a> {
    type Item = &'a TreeEntry;
    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < TREE_BUCKETS {
            let b = &self.tree.buckets[self.idx];
            self.idx = self.idx.wrapping_add(1);
            if b.is_occupied() {
                return Some(b);
            }
        }
        None
    }
}

/// Reference wrapper for sorted collection.
pub struct TreeEntryRef<'a> {
    pub entry: &'a TreeEntry,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::path_component::validate_component;
    use super::*;

    fn fake_oid(b: u8) -> ObjectId {
        let mut arr = [0u8; 32];
        arr[0] = b;
        ObjectId(arr)
    }

    #[test]
    fn test_insert_find() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"hello").test_unwrap();
        t.insert(&c, fake_oid(1), 0).test_unwrap();
        let e = t.find(&c).test_unwrap();
        assert_eq!(e.name_bytes(), b"hello");
        assert_eq!(e.oid.0[0], 1);
    }
    #[test]
    fn test_duplicate_insert() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"dup").test_unwrap();
        t.insert(&c, fake_oid(1), 0).test_unwrap();
        assert!(matches!(
            t.insert(&c, fake_oid(2), 0),
            Err(ExofsError::ObjectAlreadyExists)
        ));
    }
    #[test]
    fn test_remove() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"bye").test_unwrap();
        t.insert(&c, fake_oid(1), 0).test_unwrap();
        t.remove(&c).test_unwrap();
        assert!(t.find(&c).is_none());
        assert_eq!(t.len(), 0);
    }
    #[test]
    fn test_remove_not_found() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"x").test_unwrap();
        assert!(matches!(t.remove(&c), Err(ExofsError::ObjectNotFound)));
    }
    #[test]
    fn test_full() {
        let mut t = PathIndexTree::new();
        for i in 0u8..=191 {
            let name = [b'a' + (i % 26), b'0' + (i / 26)];
            let c = validate_component(&name).test_unwrap();
            t.insert(&c, fake_oid(i), 0).test_unwrap();
        }
        let extra = validate_component(b"extra").test_unwrap();
        assert!(matches!(
            t.insert(&extra, fake_oid(0), 0),
            Err(ExofsError::NoSpace)
        ));
    }
    #[test]
    fn test_compact() {
        let mut t = PathIndexTree::new();
        for i in 0u8..10 {
            let name = [b'a' + i];
            let c = validate_component(&name).test_unwrap();
            t.insert(&c, fake_oid(i), 0).test_unwrap();
        }
        let c = validate_component(b"a").test_unwrap();
        t.remove(&c).test_unwrap();
        t.compact().test_unwrap();
        assert_eq!(t.len(), 9);
        assert_eq!(t.tombstones, 0);
    }
    #[test]
    fn test_iter_count() {
        let mut t = PathIndexTree::new();
        for i in 0u8..5 {
            let name = [b'x' + i];
            let c = validate_component(&name).test_unwrap();
            t.insert(&c, fake_oid(i), 0).test_unwrap();
        }
        assert_eq!(t.iter().count(), 5);
    }
    #[test]
    fn test_update_oid() {
        let mut t = PathIndexTree::new();
        let c = validate_component(b"upd").test_unwrap();
        t.insert(&c, fake_oid(1), 0).test_unwrap();
        t.update_oid(&c, fake_oid(99)).test_unwrap();
        assert_eq!(t.find(&c).test_unwrap().oid.0[0], 99);
    }
}
