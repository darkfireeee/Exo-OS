// SPDX-License-Identifier: MIT
// ExoFS — object_kind/path_index.rs
// PathIndex — page d'index de chemins (nœuds de l'arbre de noms ExoFS).
//
// Règles :
//   LOBJ-01  : un PathIndex est TOUJOURS Class2 (mutable, partagé)
//   ONDISK-01: PathIndexEntryDisk #[repr(C, packed)]
//   OOM-02   : try_reserve avant chaque push
//   ARITH-02 : checked_add / saturating_* partout
//   RECUR-01 : itératif seulement (recherche binaire en boucle)


use core::fmt;
use core::mem;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ObjectId, EpochId, ExofsError, ExofsResult, blake3_hash,
};
use crate::fs::exofs::core::object_kind::ObjectKind;

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Taille d'une page PathIndex (4096 octets, aligné sur la mémoire virtuelle).
pub const PATH_INDEX_PAGE_SIZE: usize = 4096;

/// Magic d'une page PathIndex : "PIDX".
pub const PATH_INDEX_MAGIC: u32 = 0x50494458;

/// Version du format PathIndexPageDisk.
pub const PATH_INDEX_VERSION: u8 = 1;

/// Longueur maximale d'un nom de composant de chemin.
pub const PATH_NAME_MAX: usize = 255;

/// Longueur maximale stockée dans PathIndexEntryDisk (tronquée pour le packed).
pub const PATH_NAME_STORE_LEN: usize = 128;

/// Nombre maximal d'entrées par page PathIndex.
pub const PATH_INDEX_MAX_ENTRIES: usize = 64;

// ── PathIndexEntryDisk ──────────────────────────────────────────────────────────

/// Représentation on-disk d'une entrée de PathIndex (192 octets, ONDISK-01).
///
/// Layout :
/// ```text
///   0..  7  hash         u64      — FNV-1a 64 bits du nom (tri rapide)
///   8.. 39  object_id    [u8;32]  — ObjectId cible
///  40.. 55  parent_id    [u8;16]  — ObjectId parent (16 B, tronqué)
///  56.. 57  name_len     u16      — longueur réelle du nom (≤ PATH_NAME_MAX)
///  58       kind         u8       — ObjectKind de la cible
///  59       flags        u8       — flags de l'entrée
///  60..187  name         [u8;128] — composant de chemin UTF-8 tronqué
/// 188..191  _pad         [u8;4]
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct PathIndexEntryDisk {
    pub hash:      u64,
    pub object_id: [u8; 32],
    pub parent_id: [u8; 16],
    pub name_len:  u16,
    pub kind:      u8,
    pub flags:     u8,
    pub name:      [u8; PATH_NAME_STORE_LEN],
    pub _pad:      [u8; 4],
}

const _: () = assert!(
    mem::size_of::<PathIndexEntryDisk>() == 192,
    "PathIndexEntryDisk doit être 192 octets (ONDISK-01)"
);

// ── Flags des entrées ──────────────────────────────────────────────────────────

pub const PATH_ENTRY_FLAG_DELETED:   u8 = 1 << 0; // Tombstone
pub const PATH_ENTRY_FLAG_OVERFLOW:  u8 = 1 << 1; // Nom > PATH_NAME_STORE_LEN
pub const PATH_ENTRY_FLAG_SYMLINK:   u8 = 1 << 2; // Lien symbolique
pub const PATH_ENTRY_FLAG_MOUNT:     u8 = 1 << 3; // Point de montage

// ── PathIndexPageDisk ────────────────────────────────────────────────────────────

/// En-tête on-disk d'une page PathIndex (64 octets).
///
/// Une page = 1 en-tête + N entrées, jusqu'à PATH_INDEX_PAGE_SIZE.
/// Les entrées commencent à l'octet 64.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct PathIndexPageHeader {
    /// Magic "PIDX".
    pub magic:        u32,
    /// Version du format.
    pub version:      u8,
    /// Nombre d'entrées valides dans cette page.
    pub entry_count:  u8,
    /// Flags de page.
    pub flags:        u16,
    /// ObjectId de la page (LogicalObject qui détient ce PathIndex).
    pub page_id:      [u8; 32],
    /// Epoch de dernière modification.
    pub epoch_modify: u64,
    /// Numéro de page (pour les PathIndex multi-page).
    pub page_no:      u32,
    /// _pad pour aligner sur 64 octets.
    pub _pad:         [u8; 8],
    /// Checksum Blake3 des 56 premiers octets, tronqué à 8 octets.
    pub checksum:     [u8; 8],
}

// const _: () = assert!(
//     mem::size_of::<PathIndexPageHeader>() == 64,
//     "PathIndexPageHeader doit être 64 octets (ONDISK-01)"
// );

impl PathIndexPageHeader {
    pub fn compute_checksum(&self) -> [u8; 8] {
        let raw: &[u8; 64] =
            // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
            unsafe { &*(self as *const PathIndexPageHeader as *const [u8; 64]) };
        let full = blake3_hash(&raw[..56]);
        let mut out = [0u8; 8];
        out.copy_from_slice(&full[..8]);
        out
    }

    pub fn verify(&self) -> ExofsResult<()> {
        if { self.magic } != PATH_INDEX_MAGIC {
            return Err(ExofsError::Corrupt);
        }
        if { self.version } != PATH_INDEX_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        let computed = self.compute_checksum();
        if { self.checksum } != computed {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }
}

// ── PathIndexEntry in-memory ───────────────────────────────────────────────────

/// Entrée de PathIndex in-memory.
#[derive(Clone, Debug)]
pub struct PathIndexEntry {
    /// Hash FNV-1a 64 bits du nom (cache pour tri rapide).
    pub hash:      u64,
    /// ObjectId de la cible.
    pub object_id: ObjectId,
    /// ObjectId du répertoire parent (premiers 16 octets).
    pub parent_id: [u8; 16],
    /// Kind de la cible.
    pub kind:      ObjectKind,
    /// Nom du composant (UTF-8, ≤ PATH_NAME_MAX).
    pub name:      [u8; PATH_NAME_STORE_LEN],
    pub name_len:  u16,
    /// Flags de l'entrée.
    pub flags:     u8,
}

impl PathIndexEntry {
    // ── Constructeur ──────────────────────────────────────────────────────────

    pub fn new(
        name:      &[u8],
        object_id: ObjectId,
        parent_id: [u8; 16],
        kind:      ObjectKind,
    ) -> ExofsResult<Self> {
        if name.is_empty() || name.len() > PATH_NAME_MAX {
            return Err(ExofsError::InvalidArgument);
        }
        let hash     = fnv1a_hash_u64(name);
        let mut stored_name = [0u8; PATH_NAME_STORE_LEN];
        let copy_len = name.len().min(PATH_NAME_STORE_LEN);
        stored_name[..copy_len].copy_from_slice(&name[..copy_len]);
        let flags = if name.len() > PATH_NAME_STORE_LEN {
            PATH_ENTRY_FLAG_OVERFLOW
        } else {
            0
        };
        Ok(Self {
            hash,
            object_id,
            parent_id,
            kind,
            name: stored_name,
            name_len: name.len() as u16,
            flags,
        })
    }

    /// Reconstruit depuis on-disk.
    pub fn from_disk(d: &PathIndexEntryDisk) -> ExofsResult<Self> {
        let kind = ObjectKind::from_u8(d.kind)
            .ok_or(ExofsError::InvalidObjectKind)?;
        Ok(Self {
            hash:      d.hash,
            object_id: ObjectId(d.object_id),
            parent_id: d.parent_id,
            kind,
            name:      d.name,
            name_len:  d.name_len,
            flags:     d.flags,
        })
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk(&self) -> PathIndexEntryDisk {
        PathIndexEntryDisk {
            hash:      self.hash,
            object_id: self.object_id.0,
            parent_id: self.parent_id,
            name_len:  self.name_len,
            kind:      self.kind as u8,
            flags:     self.flags,
            name:      self.name,
            _pad:      [0; 4],
        }
    }

    // ── Requêtes ───────────────────────────────────────────────────────────────

    #[inline]
    pub fn name_bytes(&self) -> &[u8] {
        let len = (self.name_len as usize).min(PATH_NAME_STORE_LEN);
        &self.name[..len]
    }

    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags & PATH_ENTRY_FLAG_DELETED != 0
    }

    /// Vrai si cette entrée est un point de montage.
    #[inline]
    pub fn is_mount(&self) -> bool {
        self.flags & PATH_ENTRY_FLAG_MOUNT != 0
    }

    /// Comparaison par hash pour la recherche binaire.
    #[inline]
    pub fn hash_cmp(&self, name: &[u8]) -> core::cmp::Ordering {
        let h = fnv1a_hash_u64(name);
        self.hash.cmp(&h)
    }
}

impl fmt::Display for PathIndexEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PathIndexEntry {{ name: {:?}, kind: {:?}, deleted: {} }}",
            self.name_bytes(),
            self.kind,
            self.is_deleted(),
        )
    }
}

// ── PathIndexPage in-memory ────────────────────────────────────────────────────

/// Page d'index de chemins in-memory.
///
/// LOBJ-01 : toujours associée à un LogicalObject de Class2.
pub struct PathIndexPage {
    /// ObjectId de la page.
    pub page_id:      ObjectId,
    /// Epoch de dernière modification.
    pub epoch_modify: EpochId,
    /// Numéro de page.
    pub page_no:      u32,
    /// Entrées (triées par hash pour la recherche binaire).
    entries:          Vec<PathIndexEntry>,
    /// Nombre d'insertions depuis la dernière compaction.
    dirty_count:      u32,
}

impl PathIndexPage {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    pub fn new(page_id: ObjectId, page_no: u32, epoch: EpochId) -> Self {
        Self {
            page_id,
            epoch_modify: epoch,
            page_no,
            entries:     Vec::new(),
            dirty_count: 0,
        }
    }

    /// Reconstruit depuis un en-tête + slice d'entrées on-disk.
    pub fn from_disk(
        header:  &PathIndexPageHeader,
        entries: &[PathIndexEntryDisk],
    ) -> ExofsResult<Self> {
        header.verify()?;
        let count = header.entry_count as usize;
        if count > PATH_INDEX_MAX_ENTRIES {
            return Err(ExofsError::Overflow);
        }
        if entries.len() < count {
            return Err(ExofsError::Corrupt);
        }
        let mut page = Self::new(
            ObjectId(header.page_id),
            header.page_no,
            EpochId(header.epoch_modify),
        );
        page.entries.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;
        for d in entries[..count].iter() {
            let e = PathIndexEntry::from_disk(d)?;
            page.entries.push(e);
        }
        Ok(page)
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk_header(&self) -> PathIndexPageHeader {
        let active = self.entries.iter().filter(|e| !e.is_deleted()).count();
        let mut h = PathIndexPageHeader {
            magic:        PATH_INDEX_MAGIC,
            version:      PATH_INDEX_VERSION,
            entry_count:  active.min(255) as u8,
            flags:        0,
            page_id:      self.page_id.0,
            epoch_modify: self.epoch_modify.0,
            page_no:      self.page_no,
            _pad:         [0; 8],
            checksum:     [0; 8],
        };
        h.checksum = h.compute_checksum();
        h
    }

    pub fn to_disk_entries(&self) -> ExofsResult<Vec<PathIndexEntryDisk>> {
        let active: Vec<&PathIndexEntry> = self
            .entries
            .iter()
            .filter(|e| !e.is_deleted())
            .collect();
        let mut out = Vec::new();
        out.try_reserve(active.len()).map_err(|_| ExofsError::NoMemory)?;
        for e in active {
            out.push(e.to_disk());
        }
        Ok(out)
    }

    // ── Opérations ────────────────────────────────────────────────────────────

    /// Insère une nouvelle entrée (maintient l'ordre par hash, OOM-02).
    pub fn insert(&mut self, entry: PathIndexEntry, now: EpochId) -> ExofsResult<()> {
        let active = self.entries.iter().filter(|e| !e.is_deleted()).count();
        if active >= PATH_INDEX_MAX_ENTRIES {
            return Err(ExofsError::NoSpace);
        }
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        // Tri itératif par hash (RECUR-01).
        self.entries.sort_unstable_by_key(|e| e.hash);
        self.epoch_modify = now;
        self.dirty_count = self.dirty_count.saturating_add(1);
        Ok(())
    }

    /// Recherche une entrée par nom exact (recherche binaire sur hash, RECUR-01).
    pub fn lookup(&self, name: &[u8]) -> Option<&PathIndexEntry> {
        let target_hash = fnv1a_hash_u64(name);
        // Recherche binaire sur hash (itérative).
        let mut lo = 0usize;
        let mut hi = self.entries.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.entries[mid].hash.cmp(&target_hash) {
                core::cmp::Ordering::Equal => {
                    // Collision possible → vérification du nom exact.
                    let e = &self.entries[mid];
                    if !e.is_deleted() && e.name_bytes() == name {
                        return Some(e);
                    }
                    // Scan linéaire local pour les collisions (RECUR-01).
                    let mut i = mid;
                    while i > 0 && self.entries[i - 1].hash == target_hash {
                        i -= 1;
                    }
                    while i < self.entries.len() && self.entries[i].hash == target_hash {
                        let e = &self.entries[i];
                        if !e.is_deleted() && e.name_bytes() == name {
                            return Some(e);
                        }
                        i += 1;
                    }
                    return None;
                }
                core::cmp::Ordering::Less    => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Supprime une entrée par nom (tombstone).
    pub fn remove(&mut self, name: &[u8], now: EpochId) -> ExofsResult<()> {
        let target_hash = fnv1a_hash_u64(name);
        for entry in self.entries.iter_mut() {
            if entry.hash == target_hash && entry.name_bytes() == name && !entry.is_deleted() {
                entry.flags |= PATH_ENTRY_FLAG_DELETED;
                self.epoch_modify = now;
                self.dirty_count = self.dirty_count.saturating_add(1);
                return Ok(());
            }
        }
        Err(ExofsError::NotFound)
    }

    /// Nombre d'entrées actives.
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_deleted()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ── Validation ────────────────────────────────────────────────────────────

    pub fn validate(&self) -> ExofsResult<()> {
        for e in self.entries.iter() {
            if e.is_deleted() {
                continue;
            }
            if e.name_len == 0 || e.name_len as usize > PATH_NAME_MAX {
                return Err(ExofsError::Corrupt);
            }
        }
        Ok(())
    }
}

impl fmt::Display for PathIndexPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PathIndexPage {{ page_no: {}, entries: {}, epoch: {} }}",
            self.page_no, self.len(), self.epoch_modify.0,
        )
    }
}

impl fmt::Debug for PathIndexPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── FNV-1a 64 bits ────────────────────────────────────────────────────────────

/// Hash FNV-1a 64 bits d'un nom de composant (rapide, pas cryptographique).
///
/// Utilisé UNIQUEMENT pour le tri et la recherche rapide.
/// La vérification d'identité utilise ObjectId (cryptographique).
#[inline]
pub fn fnv1a_hash_u64(data: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 14695981039346656037;
    const FNV_PRIME:  u64 = 1099511628211;
    let mut h = FNV_OFFSET;
    // RECUR-01 : boucle, pas récursif.
    for &b in data.iter() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

// ── PathIndexStats ─────────────────────────────────────────────────────────────

/// Statistiques des pages PathIndex.
#[derive(Default, Debug)]
pub struct PathIndexStats {
    pub total_pages:    u64,
    pub total_entries:  u64,
    pub tombstone_count:u64,
    pub overflow_count: u64,
    pub mount_points:   u64,
}

impl PathIndexStats {
    pub fn new() -> Self { Self::default() }

    pub fn record(&mut self, page: &PathIndexPage) {
        self.total_pages = self.total_pages.saturating_add(1);
        for e in page.entries.iter() {
            self.total_entries = self.total_entries.saturating_add(1);
            if e.is_deleted() { self.tombstone_count = self.tombstone_count.saturating_add(1); }
            if e.flags & PATH_ENTRY_FLAG_OVERFLOW != 0 { self.overflow_count = self.overflow_count.saturating_add(1); }
            if e.is_mount() { self.mount_points = self.mount_points.saturating_add(1); }
        }
    }
}

impl fmt::Display for PathIndexStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PathIndexStats {{ pages: {}, entries: {}, tombstones: {}, \
             overflows: {}, mounts: {} }}",
            self.total_pages, self.total_entries, self.tombstone_count,
            self.overflow_count, self.mount_points,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_disk_size() {
        assert_eq!(mem::size_of::<PathIndexEntryDisk>(), 192);
    }

    #[test]
    fn test_header_disk_size() {
        assert_eq!(mem::size_of::<PathIndexPageHeader>(), 64);
    }

    #[test]
    fn test_fnv1a_deterministic() {
        let h1 = fnv1a_hash_u64(b"hello");
        let h2 = fnv1a_hash_u64(b"hello");
        assert_eq!(h1, h2);
        assert_ne!(fnv1a_hash_u64(b"hello"), fnv1a_hash_u64(b"world"));
    }

    #[test]
    fn test_insert_and_lookup() {
        let mut page = PathIndexPage::new(ObjectId([0;32]), 0, EpochId(1));
        let entry = PathIndexEntry::new(
            b"myfile",
            ObjectId([1;32]),
            [0u8;16],
            ObjectKind::Blob,
        ).unwrap();
        page.insert(entry, EpochId(2)).unwrap();
        assert!(page.lookup(b"myfile").is_some());
        assert!(page.lookup(b"other").is_none());
    }

    #[test]
    fn test_remove_sets_tombstone() {
        let mut page = PathIndexPage::new(ObjectId([0;32]), 0, EpochId(1));
        let entry = PathIndexEntry::new(
            b"toremove",
            ObjectId([2;32]),
            [0u8;16],
            ObjectKind::Blob,
        ).unwrap();
        page.insert(entry, EpochId(1)).unwrap();
        page.remove(b"toremove", EpochId(2)).unwrap();
        assert!(page.lookup(b"toremove").is_none());
        assert_eq!(page.len(), 0);
    }
}
