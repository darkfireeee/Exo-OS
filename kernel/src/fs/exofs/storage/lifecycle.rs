//! ExoFS storage lifecycle (`storage_init`, `storage_shutdown`), health reporting, and tests.
//!
//! Extracted from `exofs/storage/mod.rs` so `mod.rs` remains a pure wiring facade.

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{superblock, StorageStatsSnapshot, SuperblockManager, STORAGE_STATS};

static STORAGE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static STORAGE_SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct StorageInitResult {
    pub superblock_epoch: u64,
    pub free_bytes: u64,
    pub object_count: u64,
    pub blob_count: u64,
    pub disk_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInitError {
    AlreadyInitialized,
    InvalidDisk,
    SuperblockCorrupted,
    DiskTooSmall,
    AllMirrorsFailed,
}

pub fn storage_init<ReadFn>(
    disk_size: u64,
    read_fn: ReadFn,
) -> Result<StorageInitResult, StorageInitError>
where
    ReadFn: Fn(
        crate::fs::exofs::core::DiskOffset,
        usize,
    ) -> crate::fs::exofs::core::ExofsResult<Vec<u8>>,
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

pub fn storage_shutdown() {
    STORAGE_SHUTDOWN.store(true, Ordering::SeqCst);
    STORAGE_INITIALIZED.store(false, Ordering::SeqCst);
}

#[inline]
pub fn storage_is_ready() -> bool {
    STORAGE_INITIALIZED.load(Ordering::Acquire) && !STORAGE_SHUTDOWN.load(Ordering::Acquire)
}

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
    pub fn collect() -> Self {
        let snap = STORAGE_STATS.snapshot();
        let total_ops = snap.blobs_created.saturating_add(snap.blobs_deleted);
        let dedup_pct = if total_ops > 0 {
            snap.dedup_hits.saturating_mul(100) / total_ops
        } else {
            0
        };

        let compress_ratio_pct = if snap.compress_bytes_in > 0 {
            let saved = snap
                .compress_bytes_in
                .saturating_sub(snap.compress_bytes_out);
            saved.saturating_mul(100) / snap.compress_bytes_in
        } else {
            0
        };

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

    pub fn is_healthy(&self) -> bool {
        self.is_ready && self.checksum_errors == 0
    }
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::{
        verify_blob_range, BlobReader, BlobVerifyMode, BlobWriter, BlobWriterConfig,
        CompressionType, ObjectType, ObjectWriter, ObjectWriterConfig, BLOB_HEADER_MAGIC,
        BLOCK_SIZE, EXOFS_MAGIC, HEAP_START_OFFSET, OBJECT_HEADER_MAGIC,
    };
    use super::*;
    use crate::fs::exofs::core::{DiskOffset, EpochId, ObjectId};
    use alloc::vec;

    const DISK_SZ: u64 = 32 * 1024 * 1024;

    fn make_formatted_disk() -> Vec<u8> {
        let mut disk = vec![0u8; DISK_SZ as usize];
        let _ = SuperblockManager::format(DISK_SZ, b"TestVol", [0xAB; 16], 0, |off, buf| {
            let s = off.0 as usize;
            if s + buf.len() <= disk.len() {
                disk[s..s + buf.len()].copy_from_slice(buf);
            }
            Ok(buf.len())
        })
        .test_unwrap();
        disk
    }

    #[test]
    fn all_modules_declared() {
        let _ = BLOCK_SIZE;
        let _ = HEAP_START_OFFSET;
        let _ = EXOFS_MAGIC;
        let _ = BLOB_HEADER_MAGIC;
        let _ = OBJECT_HEADER_MAGIC;
    }

    #[test]
    fn storage_is_ready_false_before_init() {
        let _ = storage_is_ready();
    }

    #[test]
    fn health_report_collects() {
        let report = StorageHealthReport::collect();
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
            v[..e - s].copy_from_slice(&disk[s..e]);
            Ok(v)
        })
        .test_unwrap();
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
            data,
            &config,
            |_| Ok(DiskOffset(0)),
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() {
                    disk[s..s + buf.len()].copy_from_slice(buf);
                }
                Ok(buf.len())
            },
            |_| None,
        )
        .test_unwrap();

        assert_eq!(result.original_size, data.len() as u64);
        assert!(!result.dedup_hit);

        let read_result = BlobReader::read_blob(
            DiskOffset(0),
            |off, sz| {
                let s = off.0 as usize;
                Ok(disk[s..s + sz].to_vec())
            },
            BlobVerifyMode::Full,
        )
        .test_unwrap();

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
            oid,
            data,
            &config,
            |n| {
                let o = DiskOffset(off);
                off += n * BLOCK_SIZE as u64;
                Ok(o)
            },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() {
                    disk[s..s + buf.len()].copy_from_slice(buf);
                }
                Ok(buf.len())
            },
            |_| None,
        )
        .test_unwrap();

        assert_eq!(result.content_size, data.len() as u64);
        assert_eq!(result.blob_count, 1);
    }

    #[test]
    fn storage_stats_not_zero_after_writes() {
        let _ = STORAGE_STATS.snapshot();
    }

    #[test]
    fn compression_type_reexport() {
        let ct = CompressionType::Lz4;
        assert_ne!(ct, CompressionType::None);
    }

    #[test]
    fn verify_blob_range_empty() {
        let disk = vec![0u8; 4096usize];
        let report = verify_blob_range(DiskOffset(0), DiskOffset(0), &|off, sz| {
            let s = off.0 as usize;
            Ok(disk[s..s + sz].to_vec())
        });
        assert_eq!(report.checked, 0);
    }

    #[test]
    fn health_report_compression_ratio() {
        let report = StorageHealthReport::collect();
        assert!(report.compress_ratio_pct <= 100);
    }
}
