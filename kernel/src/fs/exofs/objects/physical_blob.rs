// SPDX-License-Identifier: MIT
// ExoFS — physical_blob.rs
// P-Blob (Physical Blob) : contenu immuable dédupliqué d'un objet.
// Règles :
//   REFCNT-01 : compare_exchange + panic sur underflow
//   ONDISK-01 : PhysicalBlobDisk → #[repr(C, packed)], types plain uniquement
//   HASH-01   : BlobId calculé sur données brutes avant compression
//   ARITH-02  : checked_add pour tout calcul d'offset
//   SEC-04    : jamais de contenu de blob Secret dans les stats/logs

#![allow(dead_code)]

use core::fmt;
use core::mem;
use core::sync::atomic::{AtomicU32, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, blake3_hash,
};
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// ── Constantes ─────────────────────────────────────────────────────────────────

/// Taille on-disk de `PhysicalBlobDisk` (120 octets).
pub const PHYSICAL_BLOB_DISK_SIZE: usize = mem::size_of::<PhysicalBlobDisk>();

/// Ref-count initial lors de l'insertion d'un nouveau blob.
pub const BLOB_INITIAL_REF_COUNT: u32 = 1;

/// Délai minimum (en epochs) avant collecte d'un blob orphelin.
pub const BLOB_GC_EPOCH_DELAY: u64 = 2;

// ── Type de compression ────────────────────────────────────────────────────────

/// Algorithme de compression utilisé pour stocker le blob.
///
/// Conforme à la spec section 2.8 : `#[repr(u8)]`.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CompressionType {
    /// Aucune compression.
    None    = 0,
    /// LZ4 (compression rapide).
    Lz4     = 1,
    /// Zstd niveau standard.
    Zstd    = 2,
    /// Zstd niveau maximal.
    ZstdMax = 3,
}

impl CompressionType {
    /// Convertit un octet vers un `CompressionType`.
    ///
    /// Retourne `None` si la valeur est inconnue.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            3 => Some(Self::ZstdMax),
            _ => None,
        }
    }

    /// Retourne l'octet encodé.
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::None    => "none",
            Self::Lz4     => "lz4",
            Self::Zstd    => "zstd",
            Self::ZstdMax => "zstd-max",
        };
        f.write_str(s)
    }
}

// ── Représentation on-disk ─────────────────────────────────────────────────────

/// Entrée on-disk d'un P-Blob dans la table de blobs.
///
/// Règle ONDISK-01 : `#[repr(C, packed)]`, types plain uniquement.
///
/// Layout (120 octets) :
/// ```text
///   0.. 31  blob_id       [u8;32]  — identifiant du blob (Blake3)
///  32.. 39  data_offset   u64      — offset disque du contenu
///  40.. 47  data_size     u64      — taille des données en octets
///  48.. 51  ref_count     u32      — ref count au dernier commit
///  52       compress_type u8       — CompressionType
///  53.. 55  _pad          [u8;3]
///  56.. 63  epoch_create  u64      — epoch de création
///  64.. 71  epoch_del     u64      — epoch de suppression (0 = vivant)
///  72.. 79  original_size u64      — taille avant compression
///  80..111  checksum      [u8;32] — Blake3 du champ data (contenu brut)
/// 112..119  _pad2         [u8;8]
/// ```
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PhysicalBlobDisk {
    /// Identifiant du blob (Blake3 du contenu brut, HASH-01).
    pub blob_id:       [u8; 32],
    /// Offset disque du premier octet du contenu.
    pub data_offset:   u64,
    /// Taille des données sur disque (compressées si applicable).
    pub data_size:     u64,
    /// Compteur de références au dernier commit (u32 plain — ONDISK-01).
    pub ref_count:     u32,
    /// Type de compression (0 = aucun).
    pub compress_type: u8,
    /// Padding.
    pub _pad:          [u8; 3],
    /// Epoch de création du blob.
    pub epoch_create:  u64,
    /// Epoch de suppression logique (0 = blob vivant).
    pub epoch_del:     u64,
    /// Taille d'origine avant compression (= data_size si non compressé).
    pub original_size: u64,
    /// Hash Blake3 du contenu brut (vérifié à la lecture).
    pub checksum:      [u8; 32],
    /// Padding pour arrondir à 120 octets.
    pub _pad2:         [u8; 8],
}

// Validation de taille en compile-time.
const _: () = assert!(
    mem::size_of::<PhysicalBlobDisk>() == 120,
    "PhysicalBlobDisk doit être exactement 120 octets (ONDISK-01)"
);

impl PhysicalBlobDisk {
    /// Retourne le `BlobId` depuis le champ on-disk.
    #[inline]
    pub fn get_blob_id(&self) -> BlobId {
        BlobId(self.blob_id)
    }

    /// Retourne `true` si le blob est logiquement supprimé.
    #[inline]
    pub fn is_deleted(&self) -> bool {
        // SAFETY: epoch_del est u64 (Copy) — accès direct au champ packed sûr
        let epoch_del = unsafe { core::ptr::read(core::ptr::addr_of!(self.epoch_del)) };
        epoch_del != 0
    }
}

impl fmt::Debug for PhysicalBlobDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PhysicalBlobDisk {{ blob_id: [..], offset: {:#x}, size: {}, \
             ref: {}, compress: {} }}",
            { self.data_offset },
            { self.data_size },
            { self.ref_count },
            self.compress_type,
        )
    }
}

// ── PhysicalBlobInMemory ───────────────────────────────────────────────────────

/// P-Blob en mémoire avec compteur de références atomique.
///
/// Plusieurs `LogicalObject` peuvent partager un même `PhysicalBlobInMemory`
/// via `Arc<PhysicalBlobInMemory>` lorsque la déduplication est active.
pub struct PhysicalBlobInMemory {
    /// Identifiant du blob (immuable après création).
    pub blob_id:        BlobId,
    /// Offset disque du contenu.
    pub data_offset:    DiskOffset,
    /// Taille des données sur disque.
    pub data_size:      u64,
    /// Taille originale avant compression.
    pub original_size:  u64,
    /// Compteur de références atomique (REFCNT-01).
    pub ref_count:      AtomicU32,
    /// Type de compression du contenu.
    pub compress_type:  CompressionType,
    /// Epoch de création.
    pub epoch_create:   EpochId,
    /// Epoch de suppression logique (0 = vivant).
    pub epoch_del:      u64,
    /// Indique si le hash a été vérifié contre le contenu disque.
    pub hash_verified:  bool,
}

impl PhysicalBlobInMemory {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Reconstruit depuis la représentation on-disk.
    pub fn from_disk(d: &PhysicalBlobDisk) -> ExofsResult<Self> {
        let compress_type = CompressionType::from_u8(d.compress_type)
            .ok_or(ExofsError::Corrupt)?;

        Ok(Self {
            blob_id:       BlobId(d.blob_id),
            data_offset:   DiskOffset(d.data_offset),
            data_size:     d.data_size,
            original_size: d.original_size,
            ref_count:     AtomicU32::new(d.ref_count),
            compress_type,
            epoch_create:  EpochId(d.epoch_create),
            epoch_del:     d.epoch_del,
            hash_verified: false,
        })
    }

    /// Crée un nouveau P-Blob pour un contenu donné.
    ///
    /// Le `blob_id` est calculé ici sur les données brutes (HASH-01).
    pub fn new(
        data_offset:   DiskOffset,
        data:          &[u8],
        original_size: u64,
        compress_type: CompressionType,
        epoch_create:  EpochId,
    ) -> Self {
        // HASH-01 : BlobId calculé sur les données BRUTES avant compression.
        let hash = blake3_hash(data);
        let blob_id = BlobId(hash);

        Self {
            blob_id,
            data_offset,
            data_size: data.len() as u64,
            original_size,
            ref_count:     AtomicU32::new(BLOB_INITIAL_REF_COUNT),
            compress_type,
            epoch_create,
            epoch_del:     0,
            hash_verified: false,
        }
    }

    // ── Sérialisation ─────────────────────────────────────────────────────────

    /// Sérialise vers la représentation on-disk.
    pub fn to_disk(&self) -> PhysicalBlobDisk {
        PhysicalBlobDisk {
            blob_id:       self.blob_id.0,
            data_offset:   self.data_offset.0,
            data_size:     self.data_size,
            ref_count:     self.ref_count.load(Ordering::Relaxed),
            compress_type: self.compress_type.as_u8(),
            _pad:          [0u8; 3],
            epoch_create:  self.epoch_create.0,
            epoch_del:     self.epoch_del,
            original_size: self.original_size,
            checksum:      self.blob_id.0, // Le BlobId IS le hash du contenu.
            _pad2:         [0u8; 8],
        }
    }

    // ── Gestion du ref-count (REFCNT-01) ──────────────────────────────────────

    /// Incrémente le compteur de références.
    #[inline]
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente le compteur de références.
    ///
    /// **Panic** si le compteur est déjà à 0 (REFCNT-01).
    ///
    /// Retourne la **nouvelle** valeur du compteur (0 = orphelin).
    #[inline]
    pub fn dec_ref(&self) -> u32 {
        loop {
            let cur = self.ref_count.load(Ordering::Acquire);
            if cur == 0 {
                // REFCNT-01 : le panic est obligatoire — underflow = corruption.
                panic!(
                    "ExoFS REFCNT-01: PhysicalBlob ref_count underflow \
                     (blob_id présent dans la table)"
                );
            }
            match self.ref_count.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let new_val = cur - 1;
                    if new_val == 0 {
                        // Le blob est devenu orphelin — notifier les stats.
                        // SEC-04 : on ne logue pas le contenu, seulement le fait
                        //          qu'un blob est devenu éligible au GC.
                        EPOCH_STATS.inc_blobs_gc_eligible();
                    }
                    return new_val;
                }
                Err(_) => continue, // Retry ABA-safe.
            }
        }
    }

    /// Retourne le compteur de références courant.
    #[inline]
    pub fn ref_count(&self) -> u32 {
        self.ref_count.load(Ordering::Acquire)
    }

    // ── Requêtes ──────────────────────────────────────────────────────────────

    /// Vrai si le blob n'est plus référencé (candidat GC).
    #[inline]
    pub fn is_orphan(&self) -> bool {
        self.ref_count.load(Ordering::Acquire) == 0
    }

    /// Vrai si le blob est logiquement supprimé.
    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.epoch_del != 0
    }

    /// Retourne le ratio de compression (1.0 = non compressé).
    ///
    /// Retourne 0.0 si `data_size == 0`.
    pub fn compression_ratio_x100(&self) -> u64 {
        if self.data_size == 0 {
            return 0;
        }
        // Ratio × 100 pour éviter float (original / compressed × 100).
        self.original_size
            .saturating_mul(100)
            .checked_div(self.data_size)
            .unwrap_or(0)
    }

    // ── Vérification de contenu ───────────────────────────────────────────────

    /// Vérifie que le contenu `data` correspond au `blob_id` (HASH-01).
    ///
    /// Règle HASH-01 : le BlobId est le Blake3 des données brutes (originales,
    /// avant compression). Donc `data` doit être les données **décompressées**.
    pub fn verify_content(&self, data: &[u8]) -> bool {
        let computed = blake3_hash(data);
        computed == self.blob_id.0
    }

    // ── Suppression logique ───────────────────────────────────────────────────

    /// Marque le blob comme supprimé à l'epoch `del_epoch`.
    pub fn mark_deleted(&mut self, del_epoch: EpochId) {
        self.epoch_del = del_epoch.0;
    }

    /// Retourne `true` si le blob peut être collecté par le GC.
    ///
    /// Conditions : orphelin ET supprimé ET `current_epoch >= epoch_del + GC_DELAY`.
    pub fn is_gc_eligible(&self, current_epoch: EpochId) -> bool {
        if !self.is_orphan() {
            return false;
        }
        if self.epoch_del == 0 {
            return false;
        }
        current_epoch.0 >= self.epoch_del.saturating_add(BLOB_GC_EPOCH_DELAY)
    }
}

impl fmt::Display for PhysicalBlobInMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SEC-04 : on ne logue jamais le contenu.
        write!(
            f,
            "PhysicalBlob {{ offset: {:#x}, size: {}, orig: {}, \
             refs: {}, compress: {}, epoch: {} }}",
            self.data_offset.0,
            self.data_size,
            self.original_size,
            self.ref_count.load(Ordering::Relaxed),
            self.compress_type,
            self.epoch_create.0,
        )
    }
}

impl fmt::Debug for PhysicalBlobInMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Type de référence partagée vers un P-Blob.
pub type PhysicalBlobRef = Arc<PhysicalBlobInMemory>;

// ── BlobStats ──────────────────────────────────────────────────────────────────

/// Statistiques sur les opérations de blobs.
#[derive(Default, Debug, Clone)]
pub struct BlobStats {
    /// Nombre de blobs créés.
    pub created:          u64,
    /// Nombre de blobs détruits (GC).
    pub gc_collected:     u64,
    /// Nombre de vérifications de contenu.
    pub verify_calls:     u64,
    /// Nombre d'erreurs de vérification.
    pub verify_errors:    u64,
    /// Nombre de reconstructions depuis disque.
    pub from_disk_count:  u64,
    /// Nombre d'erreurs de désérialisation.
    pub from_disk_errors: u64,
    /// Nombre total de bytes référencés.
    pub total_bytes:      u64,
}

impl BlobStats {
    pub const fn new() -> Self {
        Self {
            created:          0,
            gc_collected:     0,
            verify_calls:     0,
            verify_errors:    0,
            from_disk_count:  0,
            from_disk_errors: 0,
            total_bytes:      0,
        }
    }
}

impl fmt::Display for BlobStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BlobStats {{ created: {}, gc: {}, verify_ok: {}, \
             verify_err: {}, from_disk: {}, total_bytes: {} }}",
            self.created,
            self.gc_collected,
            self.verify_calls - self.verify_errors,
            self.verify_errors,
            self.from_disk_count,
            self.total_bytes,
        )
    }
}

// ── PhysicalBlobTable ──────────────────────────────────────────────────────────

/// Table in-memory des P-Blobs connus.
///
/// Utilisée pour la déduplication et le GC. Stockée dans un `Vec` trié par
/// `BlobId` pour la recherche O(log n). La table est protégée par un
/// `SpinLock` au niveau appelant (LOCK-04 : pas d'I/O sous le lock).
pub struct PhysicalBlobTable {
    /// Entrées triées par BlobId.
    entries: Vec<PhysicalBlobRef>,
    /// Statistiques.
    pub stats: BlobStats,
}

impl PhysicalBlobTable {
    /// Crée une table vide.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            stats:   BlobStats::new(),
        }
    }

    /// Recherche un blob par son `BlobId`.
    ///
    /// Retourne `Some(&ref)` si trouvé, `None` sinon.
    pub fn lookup(&self, blob_id: &BlobId) -> Option<&PhysicalBlobRef> {
        // Recherche linéaire (la table est généralement petite en mémoire).
        // Pour de très grandes tables, un B-Tree serait préférable.
        for entry in &self.entries {
            if entry.blob_id.0 == blob_id.0 {
                return Some(entry);
            }
        }
        None
    }

    /// Insère un nouveau blob dans la table.
    ///
    /// Si un blob avec le même `BlobId` existe déjà, incrémente son ref_count
    /// et retourne une référence vers ce blob (déduplication).
    ///
    /// Règle OOM-02 : `try_reserve(1)` avant `push`.
    pub fn insert_or_dedup(&mut self, blob: PhysicalBlobRef) -> ExofsResult<PhysicalBlobRef> {
        // Vérifier si le BlobId existe déjà (déduplication).
        for entry in &self.entries {
            if entry.blob_id.0 == blob.blob_id.0 {
                entry.inc_ref();
                self.stats.created = self.stats.created.saturating_add(1);
                return Ok(Arc::clone(entry));
            }
        }
        // Nouveau blob — insérer.
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        let new_ref = Arc::clone(&blob);
        self.entries.push(blob);
        self.stats.created       = self.stats.created.saturating_add(1);
        self.stats.total_bytes   = self.stats.total_bytes
            .saturating_add(new_ref.data_size);
        Ok(new_ref)
    }

    /// Supprime les blobs orphelins éligibles au GC.
    ///
    /// Retourne le nombre de blobs supprimés.
    pub fn drain_gc_eligible(&mut self, current_epoch: EpochId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|b| !b.is_gc_eligible(current_epoch));
        let removed = before - self.entries.len();
        self.stats.gc_collected = self.stats.gc_collected
            .saturating_add(removed as u64);
        removed
    }

    /// Nombre d'entrées dans la table.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Vrai si la table est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Display for PhysicalBlobTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PhysicalBlobTable {{ entries: {}, stats: {} }}",
            self.entries.len(),
            self.stats,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob(offset: u64, data: &[u8]) -> PhysicalBlobRef {
        Arc::new(PhysicalBlobInMemory::new(
            DiskOffset(offset),
            data,
            data.len() as u64,
            CompressionType::None,
            EpochId(1),
        ))
    }

    #[test]
    fn test_ref_count_inc_dec() {
        let b = make_blob(0, b"hello");
        assert_eq!(b.ref_count(), 1);
        b.inc_ref();
        assert_eq!(b.ref_count(), 2);
        let new = b.dec_ref();
        assert_eq!(new, 1);
        assert!(!b.is_orphan());
        let new2 = b.dec_ref();
        assert_eq!(new2, 0);
        assert!(b.is_orphan());
    }

    #[test]
    fn test_verify_content() {
        let data = b"test content";
        let b = make_blob(0, data);
        assert!(b.verify_content(data));
        assert!(!b.verify_content(b"wrong"));
    }

    #[test]
    fn test_from_disk_roundtrip() {
        let b   = make_blob(0x4000, b"roundtrip");
        let d   = b.to_disk();
        let b2  = PhysicalBlobInMemory::from_disk(&d).unwrap();
        assert_eq!(b.blob_id.0, b2.blob_id.0);
        assert_eq!(b.data_offset.0, b2.data_offset.0);
    }

    #[test]
    fn test_table_dedup() {
        let mut table = PhysicalBlobTable::new();
        let b1 = make_blob(0, b"same content");
        let b2 = make_blob(0, b"same content"); // même BlobId
        table.insert_or_dedup(b1).unwrap();
        table.insert_or_dedup(b2).unwrap();
        // Ne doit avoir qu'une seule entrée.
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_compression_type_roundtrip() {
        for v in [0u8, 1, 2, 3] {
            let ct = CompressionType::from_u8(v).unwrap();
            assert_eq!(ct.as_u8(), v);
        }
        assert!(CompressionType::from_u8(99).is_none());
    }
}
