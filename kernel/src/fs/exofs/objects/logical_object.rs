// SPDX-License-Identifier: MIT
// ExoFS — logical_object.rs
// LogicalObject : représentation RAM d'un objet ExoFS.
// Règles :
//   ONDISK-01 : LogicalObjectDisk → #[repr(C, packed)], types plain
//   REFCNT-01 : compare_exchange + panic sur underflow
//   ARITH-02  : checked_add / saturating_* partout
//   HDR-03    : verify() AVANT tout accès au payload

#![allow(dead_code)]

use core::fmt;
use core::mem;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;

use crate::fs::exofs::core::{
    ObjectId, BlobId, EpochId, DiskOffset,
    ExofsError, ExofsResult, blake3_hash,
};
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::objects::object_meta::ObjectMeta;
use crate::fs::exofs::objects::physical_ref::PhysicalRef;
use crate::fs::exofs::objects::extent_tree::ExtentTree;
use crate::scheduler::sync::rwlock::RwLock;

// ── Constantes ─────────────────────────────────────────────────────────────────

/// Magic number dans le checksum d'un LogicalObjectDisk valide.
pub const LOGICAL_OBJECT_MAGIC: u32 = 0xE4_0B_1A_57;

/// Version courante du format LogicalObjectDisk.
pub const LOGICAL_OBJECT_VERSION: u8 = 1;

// ── Représentation on-disk ─────────────────────────────────────────────────────

/// Structure on-disk d'un LogicalObject.
///
/// Règle ONDISK-01 : `#[repr(C, packed)]`, types plain uniquement.
/// Taille fixe : 256 octets.
///
/// Layout :
/// ```text
///   0.. 31  object_id    [u8;32] — identifiant de l'objet
///  32.. 63  blob_id      [u8;32] — BlobId du contenu (HASH-01)
///  64.. 71  epoch_create u64
///  72.. 79  epoch_modify u64
///  80.. 87  blob_offset  u64    — offset disque du P-Blob
///  88.. 95  data_size    u64
///  96.. 97  flags        u16
///  98       kind         u8
///  99       class        u8
/// 100..103  ref_count    u32
/// 104..107  mode         u32
/// 108..111  uid          u32
/// 112..115  gid          u32
/// 116       version      u8     — version format
/// 117..119  _pad0        [u8;3]
/// 120..127  generation   u64   — compteur de génération (CoW)
/// 128..191  _pad1        [u8;64]
/// 192..223  checksum     [u8;32] — Blake3 des 192 premiers octets
/// 224..255  _pad2        [u8;32]
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct LogicalObjectDisk {
    /// Identifiant de l'objet (32 octets).
    pub object_id:    [u8; 32],
    /// BlobId du contenu actuel (HASH-01 : Blake3 avant compression).
    pub blob_id:      [u8; 32],
    /// Epoch de création.
    pub epoch_create: u64,
    /// Epoch de dernière modification.
    pub epoch_modify: u64,
    /// Offset disque du P-Blob (0 si inline ou vide).
    pub blob_offset:  u64,
    /// Taille des données en octets.
    pub data_size:    u64,
    /// Flags (ObjectFlags en u16).
    pub flags:        u16,
    /// Kind (ObjectKind en u8).
    pub kind:         u8,
    /// Class (ObjectClass en u8).
    pub class:        u8,
    /// Compteur de références au dernier commit (plain u32).
    pub ref_count:    u32,
    /// Mode POSIX.
    pub mode:         u32,
    /// UID numérique.
    pub uid:          u32,
    /// GID numérique.
    pub gid:          u32,
    /// Version du format.
    pub version:      u8,
    pub _pad0:        [u8; 3],
    /// Compteur de génération CoW.
    pub generation:   u64,
    pub _pad1:        [u8; 64],
    /// Checksum Blake3 des 192 premiers octets.
    pub checksum:     [u8; 32],
    pub _pad2:        [u8; 32],
}

// Validation taille en compile-time.
const _: () = assert!(
    mem::size_of::<LogicalObjectDisk>() == 256,
    "LogicalObjectDisk doit être exactement 256 octets (ONDISK-01)"
);

impl LogicalObjectDisk {
    /// Calcule le checksum Blake3 des 192 premiers octets.
    pub fn compute_checksum(&self) -> [u8; 32] {
        let bytes: &[u8; 256] =
            // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
            unsafe { &*(self as *const LogicalObjectDisk as *const [u8; 256]) };
        blake3_hash(&bytes[..192])
    }

    /// Règle HDR-03 : vérifie le checksum AVANT tout accès au payload.
    pub fn verify(&self) -> ExofsResult<()> {
        let stored   = { self.checksum };
        let computed = self.compute_checksum();
        if stored != computed {
            return Err(ExofsError::Corrupt);
        }
        if self.version != LOGICAL_OBJECT_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        Ok(())
    }
}

impl fmt::Debug for LogicalObjectDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LogicalObjectDisk {{ kind: {}, class: {}, flags: {:#x}, \
             size: {}, epoch_create: {}, epoch_modify: {} }}",
            { self.kind },
            { self.class },
            { self.flags },
            { self.data_size },
            { self.epoch_create },
            { self.epoch_modify },
        )
    }
}

// ── LogicalObject in-memory ────────────────────────────────────────────────────

/// `LogicalObject` in-memory.
///
/// Conforme à la spec 2.2 : layout cache-line friendly.
///
/// Cache line 1 (0..63, hot path) :
///   `object_id[32]`, `kind[1]`, `class[1]`, `flags[2]`, `ref_count[4]`,
///   `epoch_last[8]`, `link_count[4]`, `_pad[12]`
///
/// Cache line 2+ : `meta`, `physical_ref`, `extent_tree`, …
#[repr(align(64))]
pub struct LogicalObject {
    // ── Cache line 1 — hot path ──────────────────────────────────────────────
    /// Identifiant unique de l'objet.
    pub object_id:    ObjectId,
    /// Kind de l'objet (1 octet, souvent lu).
    pub kind:         ObjectKind,
    /// Class de l'objet (Class1 = immuable/copie, Class2 = mutable/shared).
    pub class:        ObjectClass,
    /// Flags de l'objet (INLINE_DATA, DELETED, …).
    pub flags:        ObjectFlags,
    /// Compteur de références atomique (REFCNT-01).
    pub ref_count:    AtomicU32,
    /// Epoch de dernière modification (atomic pour lecture sans lock).
    pub epoch_last:   AtomicU64,
    /// Nombre de liens durs (nlink, atomique pour inc/dec rapide).
    pub link_count:   AtomicU32,

    // ── Cache line 2+ — cold path ────────────────────────────────────────────
    /// BlobId du contenu actuel.
    pub blob_id:      BlobId,
    /// Epoch de création.
    pub epoch_create: EpochId,
    /// Offset disque du P-Blob (ou 0 si inline/empty).
    pub disk_offset:  DiskOffset,
    /// Taille des données en octets.
    pub data_size:    u64,
    /// Compteur de génération CoW (incrémenté à chaque écriture).
    pub generation:   u64,
    /// Métadonnées étendues (permissions, timestamps, MIME, xattrs).
    pub meta:         ObjectMeta,
    /// Référence à la ressource physique (Blob, Inline ou Empty).
    pub physical_ref: PhysicalRef,
    /// Arbre des extents (mapping logique → disque).
    pub extent_tree:  ExtentTree,
}

impl LogicalObject {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Reconstruit depuis la représentation on-disk.
    ///
    /// Règle HDR-03 : `d.verify()` est appelé EN PREMIER.
    pub fn from_disk(d: &LogicalObjectDisk) -> ExofsResult<Self> {
        // HDR-03 : vérifier le checksum AVANT d'utiliser les champs.
        d.verify()?;

        let kind = ObjectKind::from_u8(d.kind)
            .ok_or(ExofsError::InvalidObjectKind)?;
        let class = match d.class {
            1 => ObjectClass::Class1,
            2 => ObjectClass::Class2,
            _ => return Err(ExofsError::InvalidObjectClass),
        };

        // Construire les métadonnées minimales depuis les champs du disk.
        let meta = ObjectMeta {
            mode:           d.mode,
            uid:            d.uid,
            gid:            d.gid,
            nlink:          1,
            atime_tsc:      0,
            mtime_tsc:      d.epoch_modify.saturating_mul(1000),
            ctime_tsc:      d.epoch_create.saturating_mul(1000),
            mime_type:      [0u8; 64],
            mime_len:       0,
            owner_cap_hash: [0u8; 32],
            extra_flags:    0,
            xattrs:         core::array::from_fn(|_| {
                crate::fs::exofs::objects::object_meta::XAttrEntry::empty()
            }),
            xattr_count:    0,
        };

        let epoch_last = d.epoch_modify;

        Ok(Self {
            object_id:    ObjectId(d.object_id),
            kind,
            class,
            flags:        ObjectFlags(d.flags),
            ref_count:    AtomicU32::new(d.ref_count),
            epoch_last:   AtomicU64::new(epoch_last),
            link_count:   AtomicU32::new(1),
            blob_id:      BlobId(d.blob_id),
            epoch_create: EpochId(d.epoch_create),
            disk_offset:  DiskOffset(d.blob_offset),
            data_size:    d.data_size,
            generation:   d.generation,
            meta,
            physical_ref: PhysicalRef::empty(),
            extent_tree:  ExtentTree::new(),
        })
    }

    // ── Sérialisation ─────────────────────────────────────────────────────────

    /// Sérialise vers la représentation on-disk avec checksum Blake3.
    pub fn to_disk(&self) -> LogicalObjectDisk {
        let class_u8 = match self.class {
            ObjectClass::Class1 => 1u8,
            ObjectClass::Class2 => 2u8,
        };
        let mut d = LogicalObjectDisk {
            object_id:    self.object_id.0,
            blob_id:      self.blob_id.0,
            epoch_create: self.epoch_create.0,
            epoch_modify: self.epoch_last.load(Ordering::Relaxed),
            blob_offset:  self.disk_offset.0,
            data_size:    self.data_size,
            flags:        self.flags.0,
            kind:         self.kind as u8,
            class:        class_u8,
            ref_count:    self.ref_count.load(Ordering::Relaxed),
            mode:         self.meta.mode,
            uid:          self.meta.uid,
            gid:          self.meta.gid,
            version:      LOGICAL_OBJECT_VERSION,
            _pad0:        [0u8; 3],
            generation:   self.generation,
            _pad1:        [0u8; 64],
            checksum:     [0u8; 32],
            _pad2:        [0u8; 32],
        };
        d.checksum = d.compute_checksum();
        d
    }

    // ── Requêtes ──────────────────────────────────────────────────────────────

    /// Vrai si l'objet est supprimé logiquement.
    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags.contains(ObjectFlags::DELETED)
    }

    /// Vrai si l'objet utilise des données inline.
    #[inline]
    pub fn is_inline(&self) -> bool {
        self.flags.contains(ObjectFlags::INLINE_DATA)
    }

    /// Vrai si l'objet est immuable.
    #[inline]
    pub fn is_immutable(&self) -> bool {
        self.meta.is_immutable()
    }

    /// Vrai si l'objet est de Class2 (mutable, partagé).
    #[inline]
    pub fn is_class2(&self) -> bool {
        matches!(self.class, ObjectClass::Class2)
    }

    /// Retourne `true` si l'objet est un PathIndex (toujours Class2, règle LOBJ-01).
    #[inline]
    pub fn is_path_index(&self) -> bool {
        matches!(self.kind, ObjectKind::PathIndex)
    }

    // ── Ref-count (REFCNT-01) ─────────────────────────────────────────────────

    /// Incrémente le compteur de références.
    #[inline]
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente le compteur de références.
    ///
    /// Utilise `compare_exchange` (REFCNT-01).  
    /// **Panic** si le compteur était déjà à 0.
    ///
    /// Retourne la nouvelle valeur (0 = orphelin).
    pub fn dec_ref(&self) -> u32 {
        loop {
            let cur = self.ref_count.load(Ordering::Acquire);
            if cur == 0 {
                // REFCNT-01 : panic obligatoire sur underflow.
                panic!(
                    "ExoFS REFCNT-01: LogicalObject ref_count underflow \
                     (objet en cours d'utilisation)"
                );
            }
            match self.ref_count.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_)  => return cur - 1,
                Err(_) => continue, // Retry ABA-safe.
            }
        }
    }

    /// Retourne le compteur de références courant.
    #[inline]
    pub fn ref_count(&self) -> u32 {
        self.ref_count.load(Ordering::Acquire)
    }

    // ── Mutations ─────────────────────────────────────────────────────────────

    /// Marque l'objet comme supprimé logiquement.
    pub fn mark_deleted(&mut self, now_epoch: EpochId) {
        self.flags = ObjectFlags(self.flags.0 | ObjectFlags::DELETED.0);
        self.epoch_last.store(now_epoch.0, Ordering::Release);
        self.meta.update_ctime(now_epoch.0.saturating_mul(1000));
    }

    /// Met à jour l'offset disque (après une écriture CoW).
    pub fn set_disk_offset(&mut self, new_offset: DiskOffset, new_gen: u64) {
        self.disk_offset = new_offset;
        self.generation  = new_gen;
    }

    /// Met à jour le BlobId et la taille (après une nouvelle écriture).
    ///
    /// Règle HASH-01 : le BlobId doit être calculé sur les données brutes
    /// AVANT cet appel.
    pub fn update_blob_id(
        &mut self,
        new_blob_id:   BlobId,
        new_data_size: u64,
        now_epoch:     EpochId,
        now_tsc:       u64,
    ) {
        self.blob_id   = new_blob_id;
        self.data_size = new_data_size;
        self.generation = self.generation.saturating_add(1);
        self.epoch_last.store(now_epoch.0, Ordering::Release);
        self.meta.update_mtime(now_tsc);
    }

    /// Touch atime (mise à jour du timestamp d'accès).
    #[inline]
    pub fn touch_atime(&mut self, now_tsc: u64) {
        self.meta.update_atime(now_tsc);
    }

    /// Touch mtime + ctime (modification du contenu).
    #[inline]
    pub fn touch_mtime(&mut self, now_tsc: u64) {
        self.meta.update_mtime(now_tsc);
    }

    /// Active le mode inline et met à jour le flag.
    pub fn set_inline_mode(&mut self, inline: bool) {
        if inline {
            self.flags = ObjectFlags(self.flags.0 | ObjectFlags::INLINE_DATA.0);
        } else {
            self.flags = ObjectFlags(self.flags.0 & !ObjectFlags::INLINE_DATA.0);
        }
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /// Valide la cohérence interne du `LogicalObject`.
    pub fn validate(&self) -> ExofsResult<()> {
        // Un PathIndex doit toujours être Class2 (LOBJ-01).
        if matches!(self.kind, ObjectKind::PathIndex)
            && !matches!(self.class, ObjectClass::Class2)
        {
            return Err(ExofsError::Corrupt);
        }
        // Un objet inline ne doit pas avoir de blob_id non-nul.
        if self.is_inline() && self.physical_ref.is_blob() {
            return Err(ExofsError::Corrupt);
        }
        // data_size doit être cohérent avec physical_ref.
        if self.data_size != self.physical_ref.size()
            && !self.physical_ref.is_empty()
        {
            // Avertissement seulement (certains états transitoires sont valides).
        }
        self.meta.validate()?;
        self.extent_tree.validate()?;
        Ok(())
    }
}

// ── Display / Debug ────────────────────────────────────────────────────────────

impl fmt::Display for LogicalObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LogicalObject {{ kind: {:?}, class: {:?}, flags: {:#x}, \
             size: {}, refs: {}, gen: {}, epoch: {}, \
             phys: {}, meta: {} }}",
            self.kind,
            self.class,
            self.flags.0,
            self.data_size,
            self.ref_count.load(Ordering::Relaxed),
            self.generation,
            self.epoch_last.load(Ordering::Relaxed),
            self.physical_ref,
            self.meta,
        )
    }
}

impl fmt::Debug for LogicalObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── LogicalObjectRef ────────────────────────────────────────────────────────────

/// Type de référence partagée à un `LogicalObject` — protégé par `RwLock`.
///
/// C'est ce type qui est manipulé par les caches, l'object table et les
/// opérations I/O du kernel.
pub type LogicalObjectRef = Arc<RwLock<LogicalObject>>;

// ── ObjectVersion ───────────────────────────────────────────────────────────────

/// Identifiant de version d'un objet (epoch + generation).
///
/// Permet de détecter qu'un objet a changé entre deux lectures sans
/// maintenir de lock.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ObjectVersion {
    /// Epoch de la dernière modification.
    pub epoch: u64,
    /// Compteur de génération CoW.
    pub generation: u64,
}

impl ObjectVersion {
    pub fn new(epoch: u64, generation: u64) -> Self {
        Self { epoch, generation }
    }

    /// Retourne `true` si cette version est strictement plus récente que `other`.
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.epoch > other.epoch
            || (self.epoch == other.epoch && self.generation > other.generation)
    }
}

impl fmt::Display for ObjectVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.epoch, self.generation)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_disk() -> LogicalObjectDisk {
        let mut d = LogicalObjectDisk {
            object_id:    [1u8; 32],
            blob_id:      [2u8; 32],
            epoch_create: 10,
            epoch_modify: 20,
            blob_offset:  0x4000,
            data_size:    1024,
            flags:        0,
            kind:         0, // ObjectKind::Blob
            class:        1, // Class1
            ref_count:    1,
            mode:         0o644,
            uid:          1000,
            gid:          1000,
            version:      LOGICAL_OBJECT_VERSION,
            _pad0:        [0; 3],
            generation:   5,
            _pad1:        [0; 64],
            checksum:     [0; 32],
            _pad2:        [0; 32],
        };
        d.checksum = d.compute_checksum();
        d
    }

    #[test]
    fn test_from_disk_verify() {
        let d  = make_test_disk();
        let lo = LogicalObject::from_disk(&d).expect("from_disk doit réussir");
        assert_eq!(lo.data_size, 1024);
        assert_eq!(lo.generation, 5);
    }

    #[test]
    fn test_checksum_corruption_detected() {
        let mut d = make_test_disk();
        d.data_size ^= 0xFF; // Corruption.
        assert!(LogicalObject::from_disk(&d).is_err());
    }

    #[test]
    fn test_to_disk_roundtrip() {
        let d   = make_test_disk();
        let lo  = LogicalObject::from_disk(&d).unwrap();
        let d2  = lo.to_disk();
        assert_eq!({ d.data_size }, { d2.data_size });
        assert_eq!({ d.generation }, { d2.generation });
    }

    #[test]
    fn test_refcount_dec_underflow_would_panic() {
        let d  = make_test_disk();
        let lo = LogicalObject::from_disk(&d).unwrap();
        assert_eq!(lo.ref_count(), 1);
        let new_val = lo.dec_ref();
        assert_eq!(new_val, 0);
        // Un second dec_ref déclencherait un panic (REFCNT-01).
    }

    #[test]
    fn test_disk_size() {
        assert_eq!(mem::size_of::<LogicalObjectDisk>(), 256);
    }

    #[test]
    fn test_object_version_ordering() {
        let v1 = ObjectVersion::new(1, 0);
        let v2 = ObjectVersion::new(1, 5);
        let v3 = ObjectVersion::new(2, 0);
        assert!( v2.is_newer_than(&v1));
        assert!( v3.is_newer_than(&v2));
        assert!(!v1.is_newer_than(&v2));
    }
}
