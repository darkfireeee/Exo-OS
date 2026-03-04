// SPDX-License-Identifier: MIT
// ExoFS — extent.rs
// Plage de données disque contiguë (Extent).
// Règles :
//   ONDISK-01 : ObjectExtentDisk → #[repr(C, packed)], types plain uniquement
//   ARITH-02  : checked_add / saturating_* pour tout calcul d'offset
//   NO-STD-01 : core::fmt pour Display/Debug

#![allow(dead_code)]

use core::fmt;
use core::mem;
use core::cmp::Ordering;
use crate::fs::exofs::core::{ExofsError, ExofsResult, DiskOffset, Extent};

// ── Constantes ─────────────────────────────────────────────────────────────────

/// Taille on-disk d'un `ObjectExtentDisk` (32 octets, ONDISK-01).
pub const EXTENT_DISK_SIZE: usize = mem::size_of::<ObjectExtentDisk>();

/// Taille maximale d'un extent unique (16 GiB, limite de sécurité).
pub const EXTENT_MAX_LEN: u64 = 16 * 1024 * 1024 * 1024;

/// Offset logique invalide (sentinelle).
pub const EXTENT_INVALID_OFFSET: u64 = u64::MAX;

// ── Flags d'un extent ─────────────────────────────────────────────────────────

/// Bit 0 : extent creux (trou de fichier, zéros non persistés).
pub const EXTENT_FLAG_SPARSE:     u8 = 1 << 0;
/// Bit 1 : données compressées sur disque.
pub const EXTENT_FLAG_COMPRESSED: u8 = 1 << 1;
/// Bit 2 : extent Copy-on-Write (partagé entre snapshots).
pub const EXTENT_FLAG_COW:        u8 = 1 << 2;
/// Bit 3 : extent en cours d'écriture (dirty, pas encore committé).
pub const EXTENT_FLAG_DIRTY:      u8 = 1 << 3;
/// Bit 4 : données chiffrées.
pub const EXTENT_FLAG_ENCRYPTED:  u8 = 1 << 4;

// ── Représentation on-disk ─────────────────────────────────────────────────────

/// Entrée d'un extent dans l'extent tree d'un objet (on-disk).
///
/// Règle ONDISK-01 : `#[repr(C, packed)]`, types plain uniquement.  
/// Taille fixe : 32 octets.
///
/// Layout :
/// ```text
///  0.. 7  logical_offset  u64
///  8..15  disk_offset     u64
/// 16..23  len             u64
/// 24      flags           u8
/// 25..31  _pad            [u8;7]
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ObjectExtentDisk {
    /// Offset logique dans l'objet (en octets).
    pub logical_offset: u64,
    /// Offset disque du premier octet de données.
    pub disk_offset:    u64,
    /// Longueur de l'extent en octets.
    pub len:            u64,
    /// Flags (EXTENT_FLAG_*).
    pub flags:          u8,
    /// Padding pour aligner à 32 octets.
    pub _pad:           [u8; 7],
}

// Validation de taille en compile-time.
const _: () = assert!(
    mem::size_of::<ObjectExtentDisk>() == 32,
    "ObjectExtentDisk doit être exactement 32 octets (ONDISK-01)"
);

impl ObjectExtentDisk {
    // Constantes de flags (alias pour compatibilité).
    pub const FLAG_SPARSE:     u8 = EXTENT_FLAG_SPARSE;
    pub const FLAG_COMPRESSED: u8 = EXTENT_FLAG_COMPRESSED;
    pub const FLAG_COW:        u8 = EXTENT_FLAG_COW;
    pub const FLAG_DIRTY:      u8 = EXTENT_FLAG_DIRTY;
    pub const FLAG_ENCRYPTED:  u8 = EXTENT_FLAG_ENCRYPTED;

    /// Crée un nouvel extent on-disk.
    pub fn new(logical_offset: u64, disk_offset: u64, len: u64, flags: u8) -> Self {
        Self {
            logical_offset,
            disk_offset,
            len,
            flags,
            _pad: [0u8; 7],
        }
    }

    /// Retourne `true` si l'extent est sparse (trou de fichier).
    #[inline]
    pub fn is_sparse(self) -> bool {
        self.flags & Self::FLAG_SPARSE != 0
    }

    /// Retourne `true` si l'extent est compressé.
    #[inline]
    pub fn is_compressed(self) -> bool {
        self.flags & Self::FLAG_COMPRESSED != 0
    }

    /// Retourne `true` si l'extent est Copy-on-Write.
    #[inline]
    pub fn is_cow(self) -> bool {
        self.flags & Self::FLAG_COW != 0
    }

    /// Calcule l'offset logique de fin de l'extent (exclusif).
    ///
    /// Règle ARITH-02 : `checked_add`.
    pub fn logical_end(self) -> ExofsResult<u64> {
        let lo  = { self.logical_offset };
        let len = { self.len };
        lo.checked_add(len).ok_or(ExofsError::Overflow)
    }

    /// Calcule l'offset disque de fin (exclusif).
    pub fn disk_end(self) -> ExofsResult<u64> {
        let do_  = { self.disk_offset };
        let len  = { self.len };
        do_.checked_add(len).ok_or(ExofsError::Overflow)
    }

    /// Valide la cohérence de l'extent.
    ///
    /// Checks :  
    /// 1. `len > 0` (un extent nul est invalide)  
    /// 2. `len ≤ EXTENT_MAX_LEN`  
    /// 3. Pas de débordement `logical_offset + len`  
    /// 4. Pas de débordement `disk_offset + len`  
    pub fn validate(self) -> ExofsResult<()> {
        let len = { self.len };
        if len == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if len > EXTENT_MAX_LEN {
            return Err(ExofsError::InvalidArgument);
        }
        self.logical_end()?;
        if !self.is_sparse() {
            self.disk_end()?;
        }
        Ok(())
    }
}

impl fmt::Debug for ObjectExtentDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ObjectExtentDisk {{ lo: {:#x}, do: {:#x}, len: {}, flags: {:#x} }}",
            { self.logical_offset },
            { self.disk_offset },
            { self.len },
            self.flags,
        )
    }
}

// ── ObjectExtent in-memory ─────────────────────────────────────────────────────

/// Extent d'un objet in-memory.
///
/// Combine l'offset logique dans l'objet et la plage disque physique
/// correspondante. Utilisé dans `ExtentTree` pour le mapping L-Obj → disque.
#[derive(Copy, Clone, Debug)]
pub struct ObjectExtent {
    /// Offset logique dans l'objet (en octets).
    pub logical_offset: u64,
    /// Plage disque physique (offset + longueur).
    pub physical:       Extent,
    /// Flags (EXTENT_FLAG_*).
    pub flags:          u8,
}

impl ObjectExtent {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Crée un extent in-memory.
    pub fn new(logical_offset: u64, disk_offset: DiskOffset, len: u64, flags: u8) -> Self {
        Self {
            logical_offset,
            physical: Extent { offset: disk_offset, len },
            flags,
        }
    }

    /// Construit depuis la représentation on-disk.
    pub fn from_disk(d: ObjectExtentDisk) -> Self {
        Self {
            logical_offset: d.logical_offset,
            physical: Extent {
                offset: DiskOffset(d.disk_offset),
                len:    d.len,
            },
            flags: d.flags,
        }
    }

    /// Sérialise vers la représentation on-disk.
    pub fn to_disk(&self) -> ObjectExtentDisk {
        ObjectExtentDisk {
            logical_offset: self.logical_offset,
            disk_offset:    self.physical.offset.0,
            len:            self.physical.len,
            flags:          self.flags,
            _pad:           [0u8; 7],
        }
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    /// Longueur de l'extent en octets.
    #[inline]
    pub fn len(&self) -> u64 {
        self.physical.len
    }

    /// Offset logique de fin (exclusif).
    #[inline]
    pub fn logical_end(&self) -> ExofsResult<u64> {
        self.logical_offset
            .checked_add(self.physical.len)
            .ok_or(ExofsError::Overflow)
    }

    /// Offset disque de fin (exclusif).
    #[inline]
    pub fn disk_end(&self) -> ExofsResult<u64> {
        self.physical.offset.0
            .checked_add(self.physical.len)
            .ok_or(ExofsError::Overflow)
    }

    // ── Flags ─────────────────────────────────────────────────────────────────

    #[inline] pub fn is_sparse(&self)     -> bool { self.flags & EXTENT_FLAG_SPARSE     != 0 }
    #[inline] pub fn is_compressed(&self) -> bool { self.flags & EXTENT_FLAG_COMPRESSED != 0 }
    #[inline] pub fn is_cow(&self)        -> bool { self.flags & EXTENT_FLAG_COW        != 0 }
    #[inline] pub fn is_dirty(&self)      -> bool { self.flags & EXTENT_FLAG_DIRTY      != 0 }
    #[inline] pub fn is_encrypted(&self)  -> bool { self.flags & EXTENT_FLAG_ENCRYPTED  != 0 }

    /// Positionne le flag dirty (extent modifié en mémoire).
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.flags |= EXTENT_FLAG_DIRTY;
    }

    /// Efface le flag dirty (extent committé sur disque).
    #[inline]
    pub fn clear_dirty(&mut self) {
        self.flags &= !EXTENT_FLAG_DIRTY;
    }

    /// Marque l'extent comme Copy-on-Write.
    #[inline]
    pub fn mark_cow(&mut self) {
        self.flags |= EXTENT_FLAG_COW;
    }

    // ── Géométrie ─────────────────────────────────────────────────────────────

    /// Retourne `true` si l'offset logique `offset` est dans cet extent.
    #[inline]
    pub fn contains_offset(&self, offset: u64) -> bool {
        if offset < self.logical_offset {
            return false;
        }
        match self.logical_end() {
            Ok(end) => offset < end,
            Err(_)  => false,
        }
    }

    /// Retourne `true` si cet extent chevauche `other` dans l'espace logique.
    pub fn overlaps_logical(&self, other: &ObjectExtent) -> bool {
        let self_end  = match self.logical_end()  { Ok(e) => e, Err(_) => return false };
        let other_end = match other.logical_end() { Ok(e) => e, Err(_) => return false };
        self.logical_offset < other_end && other.logical_offset < self_end
    }

    /// Retourne `true` si cet extent est adjacent (juste après) à `prev`.
    ///
    /// Adjacence logique ET physique (les blocs disque sont contigus).
    pub fn is_contiguous_with(&self, prev: &ObjectExtent) -> bool {
        match (prev.logical_end(), prev.disk_end()) {
            (Ok(log_end), Ok(disk_end)) => {
                log_end == self.logical_offset
                    && disk_end == self.physical.offset.0
                    && self.flags == prev.flags
            }
            _ => false,
        }
    }

    /// Tente de fusionner `other` dans `self` si les deux sont contigus.
    ///
    /// Retourne `Ok(())` si la fusion a réussi, `Err` sinon.
    pub fn try_merge(&mut self, other: &ObjectExtent) -> ExofsResult<()> {
        if !other.is_contiguous_with(self) {
            return Err(ExofsError::InvalidArgument);
        }
        // ARITH-02 : saturating_add — la validation sera refaite par le caller.
        self.physical.len = self.physical.len
            .checked_add(other.physical.len)
            .ok_or(ExofsError::Overflow)?;
        Ok(())
    }

    /// Divise l'extent en [self, right] à l'offset logique `split_offset`.
    ///
    /// Après l'appel, `self` couvre `[logical_offset, split_offset)` et
    /// la valeur retournée couvre `[split_offset, old_end)`.
    ///
    /// Retourne `Err` si `split_offset` n'est pas strictement à l'intérieur.
    pub fn split_at(&self, split_offset: u64) -> ExofsResult<(ObjectExtent, ObjectExtent)> {
        if split_offset <= self.logical_offset {
            return Err(ExofsError::InvalidArgument);
        }
        let end = self.logical_end()?;
        if split_offset >= end {
            return Err(ExofsError::InvalidArgument);
        }
        // Calcul ARITH-02.
        let left_len  = split_offset
            .checked_sub(self.logical_offset)
            .ok_or(ExofsError::Overflow)?;
        let right_len = end
            .checked_sub(split_offset)
            .ok_or(ExofsError::Overflow)?;
        let right_disk_offset = self.physical.offset.0
            .checked_add(left_len)
            .ok_or(ExofsError::Overflow)?;

        let left  = ObjectExtent::new(
            self.logical_offset,
            self.physical.offset,
            left_len,
            self.flags,
        );
        let right = ObjectExtent::new(
            split_offset,
            DiskOffset(right_disk_offset),
            right_len,
            self.flags,
        );
        Ok((left, right))
    }

    /// Crée une copie CoW de cet extent à un nouveau `disk_offset`.
    ///
    /// Le nouvel extent hérite de tous les flags sauf `DIRTY`, et reçoit `COW`.
    pub fn cow_copy(&self, new_disk_offset: DiskOffset) -> ObjectExtent {
        let mut flags = self.flags;
        flags &= !EXTENT_FLAG_DIRTY;
        flags |=  EXTENT_FLAG_COW;
        ObjectExtent {
            logical_offset: self.logical_offset,
            physical: Extent {
                offset: new_disk_offset,
                len:    self.physical.len,
            },
            flags,
        }
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /// Valide la cohérence in-memory de cet extent.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.physical.len == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.physical.len > EXTENT_MAX_LEN {
            return Err(ExofsError::InvalidArgument);
        }
        self.logical_end()?;
        if !self.is_sparse() {
            self.disk_end()?;
        }
        Ok(())
    }
}

// ── Ordering ───────────────────────────────────────────────────────────────────

impl PartialEq for ObjectExtent {
    fn eq(&self, other: &Self) -> bool {
        self.logical_offset == other.logical_offset
    }
}

impl Eq for ObjectExtent {}

impl PartialOrd for ObjectExtent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ObjectExtent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.logical_offset.cmp(&other.logical_offset)
    }
}

// ── Display ────────────────────────────────────────────────────────────────────

impl fmt::Display for ObjectExtent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Extent[lo={:#x}, do={:#x}, len={}, flags={}{}{}{} ]",
            self.logical_offset,
            self.physical.offset.0,
            self.physical.len,
            if self.is_sparse()     { "S" } else { "-" },
            if self.is_compressed() { "C" } else { "-" },
            if self.is_cow()        { "W" } else { "-" },
            if self.is_dirty()      { "D" } else { "-" },
        )
    }
}

// ── ExtentStats ────────────────────────────────────────────────────────────────

/// Statistiques sur les opérations d'extents.
#[derive(Default, Debug, Clone)]
pub struct ExtentStats {
    /// Nombre d'extents créés.
    pub created:         u64,
    /// Nombre d'extents fusionnés.
    pub merged:          u64,
    /// Nombre d'extents divisés (split).
    pub split:           u64,
    /// Nombre de copies CoW.
    pub cow_copies:      u64,
    /// Nombre d'erreurs de validation.
    pub validate_errors: u64,
}

impl ExtentStats {
    pub const fn new() -> Self {
        Self {
            created:         0,
            merged:          0,
            split:           0,
            cow_copies:      0,
            validate_errors: 0,
        }
    }
}

impl fmt::Display for ExtentStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ExtentStats {{ created: {}, merged: {}, split: {}, \
             cow: {}, validate_err: {} }}",
            self.created,
            self.merged,
            self.split,
            self.cow_copies,
            self.validate_errors,
        )
    }
}

// ── ExtentBuilder ──────────────────────────────────────────────────────────────

/// Builder fluent pour la création d'un `ObjectExtent`.
#[derive(Default)]
pub struct ExtentBuilder {
    logical_offset: u64,
    disk_offset:    u64,
    len:            u64,
    flags:          u8,
}

impl ExtentBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn logical_offset(mut self, v: u64) -> Self {
        self.logical_offset = v;
        self
    }

    pub fn disk_offset(mut self, v: u64) -> Self {
        self.disk_offset = v;
        self
    }

    pub fn len(mut self, v: u64) -> Self {
        self.len = v;
        self
    }

    pub fn sparse(mut self) -> Self {
        self.flags |= EXTENT_FLAG_SPARSE;
        self
    }

    pub fn compressed(mut self) -> Self {
        self.flags |= EXTENT_FLAG_COMPRESSED;
        self
    }

    pub fn cow(mut self) -> Self {
        self.flags |= EXTENT_FLAG_COW;
        self
    }

    /// Construit l'`ObjectExtent` après validation.
    pub fn build(self) -> ExofsResult<ObjectExtent> {
        let ext = ObjectExtent::new(
            self.logical_offset,
            DiskOffset(self.disk_offset),
            self.len,
            self.flags,
        );
        ext.validate()?;
        Ok(ext)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extent_disk_size() {
        assert_eq!(mem::size_of::<ObjectExtentDisk>(), 32);
    }

    #[test]
    fn test_from_disk_roundtrip() {
        let d = ObjectExtentDisk::new(0x1000, 0x4000, 0x2000, EXTENT_FLAG_SPARSE);
        let e = ObjectExtent::from_disk(d);
        let d2 = e.to_disk();
        assert_eq!({ d.logical_offset }, { d2.logical_offset });
        assert_eq!({ d.len }, { d2.len });
    }

    #[test]
    fn test_contains_offset() {
        let e = ObjectExtent::new(0x1000, DiskOffset(0x4000), 0x1000, 0);
        assert!( e.contains_offset(0x1000));
        assert!( e.contains_offset(0x1FFF));
        assert!(!e.contains_offset(0x2000));
        assert!(!e.contains_offset(0x0FFF));
    }

    #[test]
    fn test_split_at() {
        let e = ObjectExtent::new(0, DiskOffset(0x8000), 0x4000, 0);
        let (left, right) = e.split_at(0x2000).unwrap();
        assert_eq!(left.physical.len,  0x2000);
        assert_eq!(right.physical.len, 0x2000);
        assert_eq!(right.logical_offset, 0x2000);
        assert_eq!(right.physical.offset.0, 0x8000 + 0x2000);
    }

    #[test]
    fn test_try_merge() {
        let mut a = ObjectExtent::new(0,      DiskOffset(0), 0x1000, 0);
        let     b = ObjectExtent::new(0x1000, DiskOffset(0x1000), 0x1000, 0);
        a.try_merge(&b).unwrap();
        assert_eq!(a.physical.len, 0x2000);
    }

    #[test]
    fn test_overlaps_logical() {
        let a = ObjectExtent::new(0,    DiskOffset(0), 0x1000, 0);
        let b = ObjectExtent::new(0x800, DiskOffset(0x800), 0x1000, 0);
        let c = ObjectExtent::new(0x1000, DiskOffset(0x1000), 0x1000, 0);
        assert!( a.overlaps_logical(&b));
        assert!(!a.overlaps_logical(&c));
    }

    #[test]
    fn test_zero_len_invalid() {
        let e = ObjectExtent::new(0, DiskOffset(0), 0, 0);
        assert!(e.validate().is_err());
    }

    #[test]
    fn test_builder() {
        let e = ExtentBuilder::new()
            .logical_offset(0x2000)
            .disk_offset(0x8000)
            .len(0x1000)
            .cow()
            .build()
            .unwrap();
        assert!(e.is_cow());
        assert_eq!(e.logical_offset, 0x2000);
    }
}
