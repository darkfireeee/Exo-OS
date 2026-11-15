//! path_index.rs — ExoFS on-disk directory index (flat B-tree sorted by hash).
//!
//! The [`PathIndex`] is the structure that maps directory entry names
//! to their [`ObjectId`]. Each ExoFS directory owns exactly one PathIndex
//! stored as the payload in its object.
//!
//! ## On-disk format
//! ```text
//! [PathIndexHeader : 148 bytes]
//! [PathIndexEntry  :  44 bytes] × entry_count
//! [name_len bytes of name for each entry, without separator]
//! ```
//!
//! ## Applied spec rules
//! - **HDR-03** : magic verified FIRST before any other field.
//! - **ARITH-02** : `checked_add` / `checked_mul` on all offsets.
//! - **OOM-02** : `try_reserve(1)` before each `Vec::push`.

extern crate alloc;
use alloc::vec::Vec;

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::path_component::{siphash_keyed, validate_component, PathComponent, NAME_MAX};
use super::path_index_tree::PathIndexTree;
use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
#[cfg(target_os = "none")]
use crate::fs::exofs::crypto::ENTROPY_POOL;

// ── Constants ────────────────────────────────────────────────────────────────

/// On-disk magic of PathIndex: `b"PIDX"`.
pub const PATH_INDEX_MAGIC: u32 = 0x50494458;
/// Current format version.
pub const PATH_INDEX_VERSION: u16 = 1;
/// Split threshold: if entry_count exceeds this value, split is recommended.
pub const PATH_INDEX_SPLIT_THRESHOLD: u32 = 192;
/// Merge threshold: if entry_count is lower, merge is considered.
pub const PATH_INDEX_MERGE_THRESHOLD: u32 = 48;

// Global SipHash mount key (PATH-01 / S-12)
static MOUNT_KEY_READY: AtomicBool = AtomicBool::new(false);
static MOUNT_KEY_INIT_LOCK: AtomicBool = AtomicBool::new(false);
static MOUNT_KEY_LO: AtomicU64 = AtomicU64::new(0);
static MOUNT_KEY_HI: AtomicU64 = AtomicU64::new(0);

/// Initializes the SipHash mount key from CSPRNG if necessary.
pub fn ensure_mount_key_initialized() {
    if MOUNT_KEY_READY.load(Ordering::Acquire) {
        return;
    }

    while MOUNT_KEY_INIT_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        if MOUNT_KEY_READY.load(Ordering::Acquire) {
            return;
        }
        core::hint::spin_loop();
    }

    if MOUNT_KEY_READY.load(Ordering::Acquire) {
        MOUNT_KEY_INIT_LOCK.store(false, Ordering::Release);
        return;
    }

    #[cfg(target_os = "none")]
    let mut key = ENTROPY_POOL.random_16();
    #[cfg(not(target_os = "none"))]
    let mut key = [0u8; 16];

    if key == [0u8; 16] {
        key[0] = 0xA5;
        key[7] = 0x5A;
        key[15] = 0xC3;
    }

    let mut lo_b = [0u8; 8];
    let mut hi_b = [0u8; 8];
    lo_b.copy_from_slice(&key[0..8]);
    hi_b.copy_from_slice(&key[8..16]);
    let lo = u64::from_le_bytes(lo_b);
    let hi = u64::from_le_bytes(hi_b);

    MOUNT_KEY_LO.store(lo, Ordering::Release);
    MOUNT_KEY_HI.store(hi, Ordering::Release);
    MOUNT_KEY_READY.store(true, Ordering::Release);
    MOUNT_KEY_INIT_LOCK.store(false, Ordering::Release);
}

/// Returns the active mount key (initialized on demand).
pub fn mount_secret_key() -> [u8; 16] {
    ensure_mount_key_initialized();
    let lo = MOUNT_KEY_LO.load(Ordering::Acquire).to_le_bytes();
    let hi = MOUNT_KEY_HI.load(Ordering::Acquire).to_le_bytes();
    let mut key = [0u8; 16];
    key[0..8].copy_from_slice(&lo);
    key[8..16].copy_from_slice(&hi);
    if key == [0u8; 16] {
        key[0] = 0xA5;
        key[7] = 0x5A;
        key[15] = 0xC3;
    }
    key
}

// ── PathIndexHeader ────────────────────────────────────────────────────────────

/// On-disk header of a PathIndex (148 bytes, `repr(C, packed)`).
///
/// **HDR-03** : `magic` is the first field and MUST be verified before any access.
/// **ONDISK-03** : no `AtomicU64` here.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PathIndexHeader {
    /// Magic `PATH_INDEX_MAGIC` — checked FIRST (HDR-03).
    pub magic: u32,
    /// Format version (currently 1).
    pub version: u16,
    /// Reserved flags.
    pub flags: u16,
    /// Parent directory OID (zero for root).
    pub parent_oid: [u8; 32],
    /// Number of entries in this index.
    pub entry_count: u32,
    /// OID of the "low" child index after split (zero if not split).
    pub split_low_oid: [u8; 32],
    /// OID of the "high" child index after split (zero if not split).
    pub split_high_oid: [u8; 32],
    /// Configured split threshold.
    pub split_threshold: u32,
    /// Alignment padding.
    pub _pad: [u8; 4],
    /// Blake3 checksum of the preceding 116 bytes.
    pub checksum: [u8; 32],
}

const _: () = assert!(size_of::<PathIndexHeader>() == 148);

impl PathIndexHeader {
    /// Creates an initialized header for a new PathIndex.
    pub fn new(parent_oid: [u8; 32]) -> Self {
        PathIndexHeader {
            magic: PATH_INDEX_MAGIC,
            version: PATH_INDEX_VERSION,
            flags: 0,
            parent_oid,
            entry_count: 0,
            split_low_oid: [0u8; 32],
            split_high_oid: [0u8; 32],
            split_threshold: PATH_INDEX_SPLIT_THRESHOLD,
            _pad: [0u8; 4],
            checksum: [0u8; 32],
        }
    }

    /// Checks magic (HDR-03).
    #[inline]
    pub fn check_magic(&self) -> ExofsResult<()> {
        // SAFETY: packed struct — copy access to avoid UB on references.
        let magic = self.magic;
        if magic != PATH_INDEX_MAGIC {
            Err(ExofsError::InvalidMagic)
        } else {
            Ok(())
        }
    }

    /// Checks magic + version.
    pub fn validate(&self) -> ExofsResult<()> {
        self.check_magic()?;
        if self.version != PATH_INDEX_VERSION {
            return Err(ExofsError::CorruptedStructure);
        }
        Ok(())
    }
}

// ── PathIndexEntry ─────────────────────────────────────────────────────────────

/// On-disk entry in a PathIndex (44 bytes, `repr(C, packed)`).
/// Followed immediately by `name_len` name bytes.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct PathIndexEntry {
    /// FNV-1a hash of name.
    pub hash: u64,
    /// OID of the object named by this entry.
    pub object_id: [u8; 32],
    /// Name length in bytes.
    pub name_len: u16,
    /// Kind (0 = Dir, 1 = File, 2 = Symlink, see ObjectKind).
    pub kind: u8,
    /// Padding.
    pub _pad: u8,
}

const _: () = assert!(size_of::<PathIndexEntry>() == 44);

impl PathIndexEntry {
    /// Total on-disk size = header 44 + name_len bytes.
    #[inline]
    pub fn on_disk_size(&self) -> usize {
        // ARITH-02
        (size_of::<PathIndexEntry>())
            .checked_add(self.name_len as usize)
            .unwrap_or(usize::MAX)
    }
}

// ── InMemoryEntry ─────────────────────────────────────────────────────────────

/// In-memory representation of a PathIndex entry.
#[derive(Clone, Debug)]
pub struct InMemoryEntry {
    pub hash: u64,
    pub oid: ObjectId,
    pub kind: u8,
    pub name: [u8; NAME_MAX + 1],
    pub name_len: u16,
}

impl InMemoryEntry {
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

// ── PathIndex ─────────────────────────────────────────────────────────────────

/// In-memory directory index, deserialized from on-disk data.
///
/// Contains a [`PathIndexTree`] for fast lookups and a `Vec<InMemoryEntry>`
/// for ordered serialization.
pub struct PathIndex {
    /// Parent directory OID.
    pub parent_oid: ObjectId,
    /// In-memory hash-table for O(1) lookups.
    tree: PathIndexTree,
    /// Entries sorted by hash for serialization.
    entries: Vec<InMemoryEntry>,
    /// Dirty flag: `true` if unserialized mutations exist.
    pub dirty: bool,
    /// Configured split threshold.
    pub split_threshold: u32,
    /// SipHash-2-4 secret key (PATH-01 / S-12).
    /// Generated from security::crypto::rng at mount time. Never [0u8; 16] in production.
    mount_key: [u8; 16],
}

impl PathIndex {
    /// Creates an empty index with SipHash key provided at mount (PATH-01 / S-12).
    pub fn new_with_key(parent_oid: ObjectId, mount_key: [u8; 16]) -> Self {
        PathIndex {
            parent_oid,
            tree: PathIndexTree::new_with_key(mount_key),
            entries: Vec::new(),
            dirty: false,
            split_threshold: PATH_INDEX_SPLIT_THRESHOLD,
            mount_key,
        }
    }

    /// Creates an empty index with null key (WARNING: tests only — PATH-02).
    pub fn new(parent_oid: ObjectId) -> Self {
        Self::new_with_key(parent_oid, mount_secret_key())
    }

    /// Number of entries in the index.
    #[inline]
    pub fn len(&self) -> usize {
        self.tree.len()
    }
    /// `true` if no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Sets the mount SipHash key and rebuilds the hash table (PATH-01).
    ///
    /// Call after `from_bytes()` to enable HashDoS protection.
    /// Rebuilds the tree with the new key — O(n).
    pub fn set_mount_key(&mut self, key: [u8; 16]) -> ExofsResult<()> {
        self.mount_key = key;
        self.tree = PathIndexTree::new_with_key(key);
        let mut i = 0usize;
        while i < self.entries.len() {
            let e = &self.entries[i];
            let name_slice = &e.name[..e.name_len as usize];
            let comp = super::path_component::validate_component(name_slice)
                .map_err(|_| ExofsError::CorruptedStructure)?;
            self.tree
                .insert(&comp, e.oid.clone(), e.kind)
                .map_err(|_| ExofsError::CorruptedStructure)?;
            i = i.wrapping_add(1);
        }
        Ok(())
    }

    // ── Deserialization ───────────────────────────────────────────────────────

    /// Deserializes a PathIndex from raw bytes.
    ///
    /// **HDR-03** : magic checked first.
    ///
    /// # Errors
    /// - [`ExofsError::InvalidMagic`]       if magic is incorrect.
    /// - [`ExofsError::CorruptedStructure`] if too short / inconsistent.
    /// - [`ExofsError::NoMemory`]           if allocation fails.
    pub fn from_bytes(data: &[u8]) -> ExofsResult<Self> {
        // ── 1. Minimum size check ─────────────────────────────────────────────
        if data.len() < size_of::<PathIndexHeader>() {
            return Err(ExofsError::CorruptedStructure);
        }

        // ── 2. Read header (copy to avoid UB on packed) ───────────────────────
        let hdr = read_header(data)?;

        // ── 3. HDR-03 : magic FIRST ──────────────────────────────────────────
        hdr.check_magic()?;
        hdr.validate()?;

        let entry_count = hdr.entry_count as usize;
        let parent_oid = ObjectId(hdr.parent_oid);
        let split_thr = hdr.split_threshold;

        // ── 4. Read entries ───────────────────────────────────────────────────
        let mut entries: Vec<InMemoryEntry> = Vec::new();
        let active_key = mount_secret_key();
        let mut tree = PathIndexTree::new_with_key(active_key);

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
            let comp =
                validate_component(name_bytes).map_err(|_| ExofsError::CorruptedStructure)?;

            let oid = ObjectId(raw_entry.object_id);

            // Insert into hash table if fixed capacity allows.
            // Beyond that, we keep the entry in `entries` and lookup falls
            // back to a bounded linear scan to preserve large directories.
            match tree.insert(&comp, oid.clone(), raw_entry.kind) {
                Ok(()) | Err(ExofsError::NoSpace) => {}
                Err(_) => return Err(ExofsError::CorruptedStructure),
            }

            // Build InMemoryEntry.
            let mut name_arr = [0u8; NAME_MAX + 1];
            name_arr[..name_bytes.len()].copy_from_slice(name_bytes);
            let ime = InMemoryEntry {
                hash: raw_entry.hash,
                oid,
                kind: raw_entry.kind,
                name: name_arr,
                name_len: raw_entry.name_len,
            };
            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(ime);

            offset = name_end;
            let _ = i; // suppress unused warning
        }

        // ── 5. Verify entry_count consistency ────────────────────────────────
        if entries.len() != entry_count {
            return Err(ExofsError::CorruptedStructure);
        }

        Ok(PathIndex {
            parent_oid,
            tree,
            entries,
            dirty: false,
            split_threshold: split_thr,
            mount_key: active_key,
        })
    }

    // ── Serialization ─────────────────────────────────────────────────────────

    /// Serializes the index to bytes for on-disk writing.
    ///
    /// Sorts entries by hash before serialization to guarantee order.
    ///
    /// # OOM-02 / ARITH-02
    pub fn serialize(&self) -> ExofsResult<Vec<u8>> {
        // Calculate total size.
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

        // Write header.
        let hdr = self.make_header();
        buf.extend_from_slice(&bytes_of_header(&hdr));

        // Sort by hash before writing.
        let mut sorted: Vec<&InMemoryEntry> = self.entries.iter().collect();
        sorted.sort_unstable_by_key(|e| e.hash);

        for e in sorted {
            let raw = PathIndexEntry {
                hash: e.hash,
                object_id: e.oid.0,
                name_len: e.name_len,
                kind: e.kind,
                _pad: 0,
            };
            buf.extend_from_slice(&bytes_of_entry(&raw));
            buf.extend_from_slice(e.name_bytes());
        }

        Ok(buf)
    }

    // ── Operations ────────────────────────────────────────────────────────────

    /// Looks up an entry by component.
    pub fn lookup(&self, comp: &PathComponent) -> Option<(ObjectId, u8)> {
        if let Some(entry) = self.tree.find(comp) {
            return Some((entry.oid.clone(), entry.kind));
        }

        self.entries
            .iter()
            .find(|entry| entry.name_bytes() == comp.as_bytes())
            .map(|entry| (entry.oid.clone(), entry.kind))
    }

    /// Inserts a new entry.
    ///
    /// # Errors
    /// - [`ExofsError::ObjectAlreadyExists`] if name exists.
    /// - [`ExofsError::NoSpace`] if index is full.
    /// - [`ExofsError::NoMemory`] if allocation fails.
    pub fn insert(&mut self, comp: &PathComponent, oid: ObjectId, kind: u8) -> ExofsResult<()> {
        if self.lookup(comp).is_some() {
            return Err(ExofsError::ObjectAlreadyExists);
        }

        match self.tree.insert(comp, oid.clone(), kind) {
            Ok(()) | Err(ExofsError::NoSpace) => {}
            Err(err) => return Err(err),
        }

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

    /// Removes an entry.
    ///
    /// # Errors
    /// - [`ExofsError::ObjectNotFound`] if name does not exist.
    pub fn remove(&mut self, comp: &PathComponent) -> ExofsResult<()> {
        let tree_result = self.tree.remove(comp);
        let name = comp.as_bytes();
        if let Some(pos) = self.entries.iter().position(|e| e.name_bytes() == name) {
            self.entries.swap_remove(pos);
            self.dirty = true;
            return Ok(());
        }

        match tree_result {
            Ok(()) => {
                self.dirty = true;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Updates the OID of an existing entry.
    pub fn update(&mut self, comp: &PathComponent, new_oid: ObjectId) -> ExofsResult<()> {
        let mut found = false;

        match self.tree.update_oid(comp, new_oid.clone()) {
            Ok(()) => found = true,
            Err(ExofsError::ObjectNotFound) => {}
            Err(err) => return Err(err),
        }

        let name = comp.as_bytes();
        for e in &mut self.entries {
            if e.name_bytes() == name {
                e.oid = new_oid.clone();
                found = true;
                break;
            }
        }

        if !found {
            return Err(ExofsError::ObjectNotFound);
        }

        self.dirty = true;
        Ok(())
    }

    /// `true` if index needs to be split.
    #[inline]
    pub fn needs_split(&self) -> bool {
        self.entries.len() as u32 >= self.split_threshold
    }

    /// `true` if index is eligible for merge.
    #[inline]
    pub fn needs_merge(&self) -> bool {
        (self.entries.len() as u32) < PATH_INDEX_MERGE_THRESHOLD
    }

    /// Returns reference to in-memory entries (for split/merge).
    pub fn entries(&self) -> &[InMemoryEntry] {
        &self.entries
    }

    /// Returns mutable reference to entries (for split/merge).
    pub fn entries_mut(&mut self) -> &mut Vec<InMemoryEntry> {
        &mut self.entries
    }

    /// Returns reference to tree (for inspection).
    pub fn tree(&self) -> &PathIndexTree {
        &self.tree
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    fn make_header(&self) -> PathIndexHeader {
        let mut hdr = PathIndexHeader::new(self.parent_oid.0);
        hdr.entry_count = self.entries.len() as u32;
        hdr.split_threshold = self.split_threshold;
        hdr
    }
}

// ── Serialization helpers (without unsafe) ────────────────────────────────────

fn read_header(data: &[u8]) -> ExofsResult<PathIndexHeader> {
    if data.len() < size_of::<PathIndexHeader>() {
        return Err(ExofsError::CorruptedStructure);
    }
    let mut hdr = PathIndexHeader::new([0u8; 32]);
    // Byte-by-byte copy, without representation casts.
    let src = &data[..size_of::<PathIndexHeader>()];
    let magic = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
    hdr.magic = magic;
    hdr.version = u16::from_le_bytes([src[4], src[5]]);
    hdr.flags = u16::from_le_bytes([src[6], src[7]]);
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
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&data[8..40]);
    let name_len = u16::from_le_bytes([data[40], data[41]]);
    let kind = data[42];
    Ok(PathIndexEntry {
        hash,
        object_id,
        name_len,
        kind,
        _pad: 0,
    })
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
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::path_component::validate_component;
    use super::*;

    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    #[test]
    fn test_new_empty() {
        let idx = PathIndex::new(fake_oid(0));
        assert!(idx.is_empty());
    }

    #[test]
    fn test_insert_lookup() {
        let mut idx = PathIndex::new(fake_oid(0));
        let c = validate_component(b"hello").test_unwrap();
        idx.insert(&c, fake_oid(1), 0).test_unwrap();
        assert_eq!(idx.len(), 1);
        let (oid, kind) = idx.lookup(&c).test_unwrap();
        assert_eq!(oid.0[0], 1);
        assert_eq!(kind, 0);
    }

    #[test]
    fn test_remove() {
        let mut idx = PathIndex::new(fake_oid(0));
        let c = validate_component(b"bye").test_unwrap();
        idx.insert(&c, fake_oid(2), 1).test_unwrap();
        idx.remove(&c).test_unwrap();
        assert!(idx.lookup(&c).is_none());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let mut idx = PathIndex::new(fake_oid(0));
        for i in 0u8..5 {
            let name = [b'a' + i];
            idx.insert(&validate_component(&name).test_unwrap(), fake_oid(i), i % 2)
                .test_unwrap();
        }
        let bytes = idx.serialize().test_unwrap();
        let idx2 = PathIndex::from_bytes(&bytes).test_unwrap();
        assert_eq!(idx2.len(), 5);
        for i in 0u8..5 {
            let c = validate_component(&[b'a' + i]).test_unwrap();
            assert!(idx2.lookup(&c).is_some());
        }
    }

    #[test]
    fn test_bad_magic() {
        let mut bytes = vec![0u8; 148];
        assert!(matches!(
            PathIndex::from_bytes(&bytes),
            Err(ExofsError::InvalidMagic)
        ));
        // Put valid magic.
        bytes[0..4].copy_from_slice(&PATH_INDEX_MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&(99u16).to_le_bytes()); // wrong version
        assert!(matches!(
            PathIndex::from_bytes(&bytes),
            Err(ExofsError::CorruptedStructure)
        ));
    }

    #[test]
    fn test_needs_split() {
        let mut idx = PathIndex::new(fake_oid(0));
        idx.split_threshold = 3;
        for i in 0u8..3 {
            idx.insert(
                &validate_component(&[b'a' + i]).test_unwrap(),
                fake_oid(i),
                0,
            )
            .test_unwrap();
        }
        assert!(idx.needs_split());
    }
}
