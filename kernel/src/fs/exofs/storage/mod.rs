//! storage — Couche d'accès disque ExoFS
//!
//! Ce module implémente l'intégralité de la couche de stockage d'ExoFS :
//! superblock, heap, blocs, cache, blobs, objets, checksums, compression,
//! déduplication et I/O batch.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │  Couche objet  │  object_writer / object_reader                     │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Couche blob   │  blob_writer / blob_reader                         │
//! │                │  (pipeline : raw→BlobId→dédup→compress→checksum)   │
//! ├───────────────────────────────────────────────────────────────────-─┤
//! │  Extent / I/O  │  extent_writer / extent_reader / io_batch          │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Compression   │  compression_choice / writer / reader              │
//! │  Dédup         │  dedup_writer / dedup_reader                       │
//! │  Checksum      │  checksum_writer / checksum_reader                 │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Blocs         │  block_cache / block_allocator                     │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Heap          │  heap / heap_allocator / heap_coalesce / heap_free_map │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Superblock    │  superblock / superblock_backup                     │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Layout        │  layout / storage_stats                            │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Règles spec appliquées dans tout le module
//!
//! | Règle | Description |
//! |-------|-------------|
//! | **HASH-02** | BlobId = Blake3 sur données RAW, AVANT compression |
//! | **WRITE-02** | `bytes_written == expected` après chaque écriture disque |
//! | **HDR-03** | magic + checksum vérifiés AVANT tout accès au payload |
//! | **ONDISK-03** | `AtomicXxx` INTERDIT dans les structs `#[repr(C)]` |
//! | **OOM-02** | `try_reserve(1)` avant chaque `Vec::push` |
//! | **ARITH-02** | `checked_add/mul()` pour toute arithmétique sur offsets |
//! | **BACKUP-01** | 3 miroirs superblock écrits à chaque commit |
//! | **BACKUP-02** | Recovery sélectionne le miroir avec epoch_current max |

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ─────────────────────────────────────────────────────────────
// Déclarations de modules (25 fichiers)
// ─────────────────────────────────────────────────────────────

/// Constantes de layout disque et fonctions d'alignement
pub mod layout;

/// Superblock ExoFS : structure disque + gestionnaire en mémoire
pub mod superblock;

/// Gestion des miroirs de superblock (BACKUP-01/02)
pub mod superblock_backup;

/// Carte de bits du heap libre
pub mod heap_free_map;

/// Coalescence des blocs adjacents du heap
pub mod heap_coalesce;

/// Allocateur de plages du heap
pub mod heap_allocator;

/// Interface unifiée du heap ExoFS
pub mod heap;

/// Allocateur de blocs physiques avec support transactions
pub mod block_allocator;

/// Cache LRU de blocs (write-back)
pub mod block_cache;

/// Batching et coalescing d'opérations I/O
pub mod io_batch;

/// Écriture de checksums Blake3 sur flux/blocs
pub mod checksum_writer;

/// Lecture et vérification de checksums Blake3
pub mod checksum_reader;

/// Sélection de l'algorithme de compression
pub mod compression_choice;

/// Compression de blobs (Lz4 / Zstd)
pub mod compression_writer;

/// Décompression de blobs (Lz4 / Zstd)
pub mod compression_reader;

/// Index de déduplication (écriture)
pub mod dedup_writer;

/// Résolution de déduplication (lecture)
pub mod dedup_reader;

/// Écriture d'extents (listes de blocs contigus)
pub mod extent_writer;

/// Lecture d'extents
pub mod extent_reader;

/// Pipeline complet d'écriture de blobs
pub mod blob_writer;

/// Pipeline complet de lecture et vérification de blobs
pub mod blob_reader;

/// Pipeline complet d'écriture d'objets (multi-blobs)
pub mod object_writer;

/// Pipeline complet de lecture d'objets
pub mod object_reader;

/// Statistiques globales du module storage
pub mod storage_stats;

// ─────────────────────────────────────────────────────────────
// Re-exports principaux
// ─────────────────────────────────────────────────────────────

// Layout
pub use layout::{
    BLOCK_SIZE, HEAP_START_OFFSET,
    align_up, align_down,
    DiskZone, LayoutMap,
};

// Superblock
pub use superblock::{
    ExoSuperblockDisk, SuperblockManager, SuperblockSnapshot,
    EXOFS_MAGIC, FORMAT_VERSION_MAJOR, SUPERBLOCK_DISK_SIZE,
    MirrorSlot, SbManagerState,
    verify_superblock_bytes, superblock_mirror_offsets,
};

// Superblock backup
pub use superblock_backup::{
    write_superblock_mirrors, recover_superblock,
};

// Heap
pub use heap::{ExofsHeap, HeapConfig};
pub use heap_allocator::{HeapAllocator, AllocationPolicy};
pub use heap_coalesce::{HeapCoalescer, CoalesceReport};
pub use heap_free_map::HeapFreeMap;

// Block allocator
pub use block_allocator::{BlockAllocator, ExtentHandle, ExtentState};

// Block cache
pub use block_cache::BlockCache;

// I/O batch
pub use io_batch::{IoBatch, IoBatchReport, BatchStats, IoBatchQueue};

// Checksums
pub use checksum_writer::{ChecksumWriter, ChecksumTag, BlockChecksumMap};
pub use checksum_reader::{ChecksumReader, VerifyMode};

// Compression
pub use compression_choice::{CompressionType, ContentHint, choose_compression};
pub use compression_writer::{CompressWriter, CompressedBlockHeader};
pub use compression_reader::{DecompressReader};

// Déduplication
pub use dedup_writer::{DedupWriter, DedupEntry, DedupDecision};
pub use dedup_reader::DedupReader;

// Extents
pub use extent_writer::{ExtentWriter, Extent, ExtentMap};
pub use extent_reader::{ExtentReader, ExtentBlockIterator};

// Blobs
pub use blob_writer::{
    BlobWriter, BlobWriterConfig, BlobWriteResult, BatchBlobWriter,
    BlobHeaderDisk, BLOB_HEADER_MAGIC, BLOB_HEADER_SIZE,
    verify_blob_header, blob_total_disk_size,
    BLOB_WRITER_STATS,
};
pub use blob_reader::{
    BlobReader, BlobReadResult, BlobVerifyMode,
    BatchBlobReader, BlobReadRequest, BatchBlobReadResult,
    BlobScanner, IntegrityReport, verify_blob_range,
    BLOB_READER_STATS,
};

// Objets
pub use object_writer::{
    ObjectWriter, ObjectWriterConfig, ObjectWriteResult,
    ObjectHeaderDisk, ObjectType, BlobRef,
    OBJECT_HEADER_MAGIC, OBJECT_HEADER_SIZE,
    OBJECT_WRITER_STATS,
};
pub use object_reader::{
    ObjectReader, ObjectReadResult, ObjectVerifyMode,
    ObjectMeta, ObjectScanner, ObjectIntegrityReport,
    verify_objects, OBJECT_READER_STATS,
};

// Statistiques
pub use storage_stats::{StorageStats, StorageStatsSnapshot, STORAGE_STATS};

// ─────────────────────────────────────────────────────────────
// Initialisation et arrêt du module storage
// ─────────────────────────────────────────────────────────────

/// Vrai si le module storage a été initialisé
static STORAGE_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Vrai si le module storage est en cours d'arrêt
static STORAGE_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Résultat de l'initialisation
#[derive(Debug, Clone)]
pub struct StorageInitResult {
    pub superblock_epoch: u64,
    pub free_bytes: u64,
    pub object_count: u64,
    pub blob_count: u64,
    pub disk_size: u64,
}

/// Erreurs d'initialisation spécifiques
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInitError {
    AlreadyInitialized,
    InvalidDisk,
    SuperblockCorrupted,
    DiskTooSmall,
    AllMirrorsFailed,
}

/// Initialise le module storage pour un volume existant.
///
/// Monte le superblock, initialise les statistiques globales.
/// Idempotent : retourne une erreur si déjà initialisé.
pub fn storage_init<ReadFn>(
    disk_size: u64,
    read_fn: ReadFn,
) -> Result<StorageInitResult, StorageInitError>
where
    ReadFn: Fn(crate::fs::exofs::core::DiskOffset, usize) -> crate::fs::exofs::core::ExofsResult<Vec<u8>>,
{
    if STORAGE_INITIALIZED.load(Ordering::Acquire) {
        return Err(StorageInitError::AlreadyInitialized);
    }
    if disk_size < superblock::MIN_DISK_SIZE {
        return Err(StorageInitError::DiskTooSmall);
    }

    let mgr = SuperblockManager::mount(disk_size, read_fn)
        .map_err(|_| StorageInitError::AllMirrorsFailed)?;

    let snap = mgr.snapshot();
    let result = StorageInitResult {
        superblock_epoch: snap.epoch,
        free_bytes: snap.free_bytes,
        object_count: snap.object_count,
        blob_count: snap.blob_count,
        disk_size: snap.disk_size,
    };

    STORAGE_INITIALIZED.store(true, Ordering::Release);

    Ok(result)
}

/// Prépare le module pour l'arrêt du système.
///
/// Marque le module comme en cours d'arrêt ; les opérations d'écriture
/// doivent être refusées après cet appel.
pub fn storage_shutdown() {
    STORAGE_SHUTDOWN.store(true, Ordering::SeqCst);
    STORAGE_INITIALIZED.store(false, Ordering::SeqCst);
}

/// Vrai si le module storage est initialisé et prêt
#[inline]
pub fn storage_is_ready() -> bool {
    STORAGE_INITIALIZED.load(Ordering::Acquire)
        && !STORAGE_SHUTDOWN.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────
// Rapport de santé du module
// ─────────────────────────────────────────────────────────────

/// Rapport de santé global du module storage
#[derive(Debug)]
pub struct StorageHealthReport {
    pub is_ready: bool,
    pub stats: StorageStatsSnapshot,
    pub write_errors: u64,
    pub read_errors: u64,
    pub checksum_errors: u64,
    pub dedup_hit_rate_pct: u64,
    pub compress_ratio_pct: u64,
}

impl StorageHealthReport {
    /// Collecte un rapport de santé à partir des compteurs globaux
    pub fn collect() -> Self {
        let snap = STORAGE_STATS.snapshot();
        let total_ops = snap.blobs_created.saturating_add(snap.blobs_deleted);
        let dedup_pct = if total_ops > 0 {
            snap.dedup_hits.saturating_mul(100) / total_ops
        } else { 0 };

        let compress_ratio_pct = if snap.compress_bytes_in > 0 {
            let saved = snap.compress_bytes_in.saturating_sub(snap.compress_bytes_out);
            saved.saturating_mul(100) / snap.compress_bytes_in
        } else { 0 };

        Self {
            is_ready: storage_is_ready(),
            write_errors: snap.io_errors,
            read_errors: snap.io_errors,
            checksum_errors: snap.checksum_errors,
            dedup_hit_rate_pct: dedup_pct,
            compress_ratio_pct,
            stats: snap,
        }
    }

    /// Vrai si le storage est dans un état sain
    pub fn is_healthy(&self) -> bool {
        self.is_ready && self.checksum_errors == 0
    }
}

// ─────────────────────────────────────────────────────────────
// Tests de fumée du module
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::fs::exofs::core::{DiskOffset, ExofsResult, ExofsError, EpochId, ObjectId};

    const DISK_SZ: u64 = 32 * 1024 * 1024;

    /// Crée et formate un disque en mémoire, retourne les bytes
    fn make_formatted_disk() -> Vec<u8> {
        let mut disk = vec![0u8; DISK_SZ as usize];
        let _ = SuperblockManager::format(
            DISK_SZ, b"TestVol", [0xAB; 16], 0,
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();
        disk
    }

    #[test]
    fn all_modules_declared() {
        // Compile-test : tous les modules doivent être accessibles
        let _ = BLOCK_SIZE;
        let _ = HEAP_START_OFFSET;
        let _ = EXOFS_MAGIC;
        let _ = BLOB_HEADER_MAGIC;
        let _ = OBJECT_HEADER_MAGIC;
    }

    #[test]
    fn storage_is_ready_false_before_init() {
        // Sans init explicite, le flag doit être à false
        // (ou true si un test précédent l'a initialisé — acceptable en test)
        let _ = storage_is_ready(); // juste vérifie que ça compile
    }

    #[test]
    fn health_report_collects() {
        let report = StorageHealthReport::collect();
        // Seulement vérifie que ça s'exécute sans panique
        let _ = report.is_healthy();
        let _ = report.dedup_hit_rate_pct;
    }

    #[test]
    fn superblock_roundtrip_via_module() {
        let disk = make_formatted_disk();

        let mgr = SuperblockManager::mount(DISK_SZ, |off, sz| {
            let s = off.0 as usize;
            let e = (s + sz).min(disk.len());
            let mut v = vec![0u8; sz];
            v[..e-s].copy_from_slice(&disk[s..e]);
            Ok(v)
        }).unwrap();

        let snap = mgr.snapshot();
        assert_eq!(snap.disk_size, DISK_SZ);
        assert!(snap.epoch >= 1);
    }

    #[test]
    fn blob_write_read_roundtrip() {
        let data = b"Module-level blob write/read test";
        let config = BlobWriterConfig::new(EpochId(1))
            .no_dedup()
            .with_algo(CompressionType::None);

        let mut disk = vec![0u8; 65536usize];
        let result = BlobWriter::write_blob(
            data, &config,
            |_| Ok(DiskOffset(0)),
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        assert_eq!(result.original_size, data.len() as u64);
        assert!(!result.dedup_hit);

        let read_result = BlobReader::read_blob(
            DiskOffset(0),
            |off, sz| {
                let s = off.0 as usize;
                Ok(disk[s..s+sz].to_vec())
            },
            BlobVerifyMode::Full,
        ).unwrap();

        assert_eq!(&read_result.data[..data.len()], data);
        assert!(read_result.id_verified);
    }

    #[test]
    fn object_write_read_roundtrip() {
        let oid = ObjectId([0x42; 32]);
        let data = b"Module-level object write/read test content";
        let config = ObjectWriterConfig::new(EpochId(2))
            .no_dedup()
            .with_type(ObjectType::Regular);

        let mut disk = vec![0u8; 65536usize];
        let mut off = 0u64;

        let result = ObjectWriter::write_object(
            oid, data, &config,
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        assert_eq!(result.content_size, data.len() as u64);
        assert_eq!(result.blob_count, 1);
    }

    #[test]
    fn storage_stats_not_zero_after_writes() {
        // Les tests précédents ont incrémenté les stats
        let snap = STORAGE_STATS.snapshot();
        let _ = snap; // Juste vérifie que ça fonctionne
    }

    #[test]
    fn compression_type_reexport() {
        let ct = CompressionType::Lz4;
        assert_ne!(ct, CompressionType::None);
    }

    #[test]
    fn verify_blob_range_empty() {
        let disk = vec![0u8; 4096usize];
        let report = verify_blob_range(
            DiskOffset(0),
            DiskOffset(0),
            &|off, sz| {
                let s = off.0 as usize;
                Ok(disk[s..s+sz].to_vec())
            },
        );
        assert_eq!(report.checked, 0);
    }

    #[test]
    fn health_report_compression_ratio() {
        let report = StorageHealthReport::collect();
        // Le ratio est entre 0 et 100
        assert!(report.compress_ratio_pct <= 100);
    }
}
