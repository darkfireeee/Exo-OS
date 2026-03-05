// SPDX-License-Identifier: MIT
// ExoFS — object_loader.rs
// ObjectLoader — reconstruction d'un LogicalObject depuis disque.
//
// Règles :
//   HDR-03   : LogicalObjectDisk.verify() AVANT toute utilisation
//   DAG-01   : PAS d'import storage/, ipc/, process/, arch/ — lecture injectée
//   ARITH-02 : checked_add / saturating_* partout
//   OOM-02   : try_reserve partout

#![allow(dead_code)]

use core::fmt;
use core::mem;
use core::sync::atomic::{AtomicU32, AtomicU64};
use alloc::sync::Arc;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, ObjectId, BlobId, EpochId, DiskOffset,
};
use crate::fs::exofs::objects::logical_object::{
    LogicalObject, LogicalObjectDisk, LogicalObjectRef,
};
use crate::fs::exofs::objects::object_meta::ObjectMeta;
use crate::fs::exofs::objects::inline_data::InlineData;
use crate::fs::exofs::objects::physical_ref::PhysicalRef;
use crate::fs::exofs::objects::extent_tree::ExtentTree;
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;
use crate::scheduler::sync::rwlock::RwLock;

// ── Type de fonction de lecture (DAG-01 : injection) ──────────────────────────

/// Type de la fonction de lecture disque injectée (DAG-01 : ni storage ni ipc).
///
/// `fn(offset, buf) -> ExofsResult<()>`
/// Lit exactement `buf.len()` octets depuis `offset` sur disque.
pub type ReadFn = fn(DiskOffset, &mut [u8]) -> ExofsResult<()>;

// ── Paramètres de chargement ───────────────────────────────────────────────────

/// Paramètres pour le chargement d'un LogicalObject.
pub struct LoadParams {
    /// Offset disque du LogicalObjectDisk.
    pub disk_offset:     DiskOffset,
    /// Vrai si les données inline doivent être vérifiées contre le BlobId.
    pub verify_content:  bool,
    /// Fonction de lecture injectée (DAG-01).
    pub read_fn:         ReadFn,
}

impl LoadParams {
    pub fn new(offset: DiskOffset, verify: bool, read_fn: ReadFn) -> Self {
        Self {
            disk_offset:    offset,
            verify_content: verify,
            read_fn,
        }
    }
}

// ── Résultat de chargement ─────────────────────────────────────────────────────

/// Résultat d'un chargement réussi.
pub struct LoadResult {
    /// Référence à l'objet chargé.
    pub object:     LogicalObjectRef,
    /// Offset disque depuis lequel il a été lu.
    pub disk_offset: DiskOffset,
    /// Taille effective lue (LogicalObjectDisk = 256 B minimum).
    pub bytes_read: usize,
}

// ── ObjectLoader ───────────────────────────────────────────────────────────────

/// Charge et reconstruit un `LogicalObject` depuis la représentation on-disk.
///
/// DAG-01 : la lecture physique est fournie par l'appelant via `LoadParams.read_fn`.
/// HDR-03 : `LogicalObjectDisk.verify()` est appelé en premier sur le buffer.
pub struct ObjectLoader;

impl ObjectLoader {
    // ── Chargement principal ───────────────────────────────────────────────────

    /// Charge un `LogicalObject` depuis son offset disque.
    ///
    /// Protocole :
    /// 1. Lire 256 octets (= `size_of::<LogicalObjectDisk>()`) via `read_fn`.
    /// 2. Vérifier le checksum et la version (HDR-03).
    /// 3. Reconstruire le `LogicalObject` in-memory.
    /// 4. Si inline + verify_content, vérifier le BlobId du payload.
    pub fn load(params: &LoadParams) -> ExofsResult<LoadResult> {
        let disk_size = mem::size_of::<LogicalObjectDisk>();

        // Buffer on-stack, initialisation explicite.
        let mut buf = [0u8; mem::size_of::<LogicalObjectDisk>()];
        assert_eq!(buf.len(), 256, "LogicalObjectDisk doit être 256 octets");

        // Lecture via la fonction injectée (DAG-01).
        (params.read_fn)(params.disk_offset, &mut buf)?;

        // SAFETY: LogicalObjectDisk est #[repr(C, packed)], buf est aligné.
        let lod: LogicalObjectDisk = unsafe {
            core::ptr::read_unaligned(buf.as_ptr() as *const LogicalObjectDisk)
        };

        // HDR-03 : vérifie checksum et version AVANT toute utilisation.
        lod.verify()?;

        // Reconstruction de l'objet in-memory.
        let obj = LogicalObject::from_disk(&lod)?;

        // Vérification optionnelle du contenu inline.
        if params.verify_content && obj.is_inline() {
            Self::verify_inline_content(&obj)?;
        }

        EPOCH_STATS.inc_objects_read();

        let arc = Arc::new(RwLock::new(obj));
        Ok(LoadResult {
            object:      arc,
            disk_offset: params.disk_offset,
            bytes_read:  disk_size,
        })
    }

    /// Charge un `LogicalObject` avec un buffer pré-rempli (évite une lecture
    /// disque si le buffer est déjà en cache).
    ///
    /// HDR-03 : `verify()` est appelé en premier sur le buffer.
    pub fn load_from_buf(
        buf:         &[u8; 256],
        disk_offset: DiskOffset,
        verify_content: bool,
    ) -> ExofsResult<LogicalObjectRef> {
        // SAFETY: LogicalObjectDisk est #[repr(C, packed)], même taille.
        let lod: LogicalObjectDisk = unsafe {
            core::ptr::read_unaligned(buf.as_ptr() as *const LogicalObjectDisk)
        };
        lod.verify()?;

        let obj = LogicalObject::from_disk(&lod)?;
        if verify_content && obj.is_inline() {
            Self::verify_inline_content(&obj)?;
        }

        EPOCH_STATS.inc_objects_read();
        Ok(Arc::new(RwLock::new(obj)))
    }

    /// Charge en batch plusieurs objets contigus sur disque.
    ///
    /// OOM-02 : alloue le vecteur en une opération.
    /// DAG-01 : toujours via `read_fn`.
    pub fn load_batch(
        base_offset: DiskOffset,
        count:       usize,
        verify:      bool,
        read_fn:     ReadFn,
    ) -> ExofsResult<alloc::vec::Vec<LoadResult>> {
        if count == 0 {
            return Ok(alloc::vec::Vec::new());
        }
        let disk_size = mem::size_of::<LogicalObjectDisk>() as u64;
        let mut results = alloc::vec::Vec::new();
        results
            .try_reserve(count)
            .map_err(|_| ExofsError::NoMemory)?;

        for i in 0..count {
            let offset = DiskOffset(
                base_offset
                    .0
                    .checked_add(i as u64 * disk_size)
                    .ok_or(ExofsError::Overflow)?,
            );
            let params = LoadParams::new(offset, verify, read_fn);
            let res = Self::load(&params)?;
            results.push(res);
        }
        Ok(results)
    }

    // ── Vérification de contenu inline ────────────────────────────────────────

    /// Vérifie que les données inline correspondent au BlobId de l'objet.
    ///
    /// HDR-03 / HASH-01 : recalcule Blake3 et compare.
    fn verify_inline_content(obj: &LogicalObject) -> ExofsResult<()> {
        if let Some(inline) = obj.physical_ref.as_inline() {
            inline.validate()?;
            let computed = crate::fs::exofs::core::compute_blob_id(
                inline.as_slice()
            );
            if computed != obj.blob_id {
                return Err(ExofsError::Corrupt);
            }
        }
        Ok(())
    }

    // ── Sérialisation ─────────────────────────────────────────────────────────

    /// Sérialise un `LogicalObject` vers son buffer on-disk pour écriture.
    ///
    /// Retourne les 256 octets avec le checksum calculé.
    pub fn serialize(obj_ref: &LogicalObjectRef) -> [u8; 256] {
        let obj  = obj_ref.read();
        let disk = obj.to_disk();
        // SAFETY: même size, même layout.
        unsafe {
            core::mem::transmute::<LogicalObjectDisk, [u8; 256]>(disk)
        }
    }
}

// ── LoaderStats ────────────────────────────────────────────────────────────────

/// Statistiques du loader.
#[derive(Default, Debug)]
pub struct LoaderStats {
    pub total_reads:      u64,
    pub verify_failures:  u64,
    pub inline_verified:  u64,
    pub batch_reads:      u64,
    pub cache_hits:       u64,
}

impl LoaderStats {
    pub fn new() -> Self { Self::default() }

    pub fn record_read(&mut self, from_cache: bool) {
        self.total_reads = self.total_reads.saturating_add(1);
        if from_cache { self.cache_hits = self.cache_hits.saturating_add(1); }
    }

    pub fn record_verify_failure(&mut self) {
        self.verify_failures = self.verify_failures.saturating_add(1);
    }
}

impl fmt::Display for LoaderStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LoaderStats {{ reads: {}, verify_fail: {}, inline_ok: {}, \
             batch: {}, cache_hits: {} }}",
            self.total_reads, self.verify_failures, self.inline_verified,
            self.batch_reads, self.cache_hits,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::objects::logical_object::LOGICAL_OBJECT_VERSION;

    fn make_disk() -> [u8; 256] {
        let mut d = LogicalObjectDisk {
            object_id:    [1u8; 32],
            blob_id:      [2u8; 32],
            epoch_create: 1,
            epoch_modify: 2,
            blob_offset:  0,
            data_size:    0,
            flags:        0,
            kind:         0,   // Blob
            class:        1,   // Class1
            ref_count:    1,
            mode:         0o644,
            uid:          0,
            gid:          0,
            version:      LOGICAL_OBJECT_VERSION,
            _pad0:        [0; 3],
            generation:   0,
            _pad1:        [0; 64],
            checksum:     [0; 32],
            _pad2:        [0; 32],
        };
        d.checksum = d.compute_checksum();
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        unsafe { core::mem::transmute::<LogicalObjectDisk, [u8; 256]>(d) }
    }

    #[test]
    fn test_load_from_buf_ok() {
        let buf = make_disk();
        let obj_ref = ObjectLoader::load_from_buf(&buf, DiskOffset(0), false).unwrap();
        let obj = obj_ref.read();
        assert_eq!(obj.data_size, 0);
    }

    #[test]
    fn test_load_from_buf_corrupt() {
        let mut buf = make_disk();
        buf[50] ^= 0xFF; // Corruption.
        let res = ObjectLoader::load_from_buf(&buf, DiskOffset(0), false);
        assert!(res.is_err());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let buf = make_disk();
        let obj_ref = ObjectLoader::load_from_buf(&buf, DiskOffset(0), false).unwrap();
        let out = ObjectLoader::serialize(&obj_ref);
        // Le checksum doit être cohérent.
        let obj2 = ObjectLoader::load_from_buf(&out, DiskOffset(0), false).unwrap();
        let o1 = obj_ref.read();
        let o2 = obj2.read();
        assert_eq!(o1.data_size, o2.data_size);
    }
}
