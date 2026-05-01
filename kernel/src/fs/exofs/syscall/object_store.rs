//! object_store.rs — Persistance simple des blobs de syscall vers block device.
//!
//! Cette couche garde un mapping `BlobId -> plage LBA` afin de permettre
//! la réouverture/relire depuis le disque même après purge du cache RAM.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use crate::fs::exofs::storage::virtio_adapter;
use crate::scheduler::sync::spinlock::SpinLock;

const DATA_LBA_START: u64 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedBlobMapping {
    pub base_lba: u64,
    pub allocated_blocks: u64,
    pub size_bytes: u64,
    pub block_size: u32,
}

struct ObjectStoreInner {
    map: BTreeMap<BlobId, PersistedBlobMapping>,
    next_lba: u64,
}

impl ObjectStoreInner {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            next_lba: 0,
        }
    }
}

pub struct ObjectStore {
    inner: SpinLock<ObjectStoreInner>,
}

pub static OBJECT_STORE: ObjectStore = ObjectStore::new_const();

impl ObjectStore {
    pub const fn new_const() -> Self {
        Self {
            inner: SpinLock::new(ObjectStoreInner::new()),
        }
    }

    pub fn lookup(&self, blob_id: &BlobId) -> Option<PersistedBlobMapping> {
        self.inner.lock().map.get(blob_id).copied()
    }

    pub fn persisted_size(&self, blob_id: &BlobId) -> Option<u64> {
        self.lookup(blob_id).map(|mapping| mapping.size_bytes)
    }

    pub fn reserve_for_write(
        &self,
        blob_id: BlobId,
        size_bytes: u64,
        block_size: u32,
        total_blocks: u64,
    ) -> ExofsResult<PersistedBlobMapping> {
        if block_size == 0 {
            return Err(ExofsError::InvalidSize);
        }

        let needed_blocks = blocks_for(size_bytes, block_size as u64)?;
        let mut inner = self.inner.lock();

        if let Some(existing) = inner.map.get_mut(&blob_id) {
            if existing.block_size != block_size {
                return Err(ExofsError::InvalidArgument);
            }
            if existing.allocated_blocks >= needed_blocks {
                existing.size_bytes = size_bytes;
                return Ok(*existing);
            }
        }

        let baseline_lba = if total_blocks > DATA_LBA_START {
            DATA_LBA_START
        } else {
            1
        };
        let start_lba = if inner.next_lba == 0 {
            baseline_lba
        } else {
            inner.next_lba.max(baseline_lba)
        };
        let end_lba = start_lba
            .checked_add(needed_blocks)
            .ok_or(ExofsError::OffsetOverflow)?;
        if end_lba > total_blocks {
            return Err(ExofsError::NoSpace);
        }

        let mapping = PersistedBlobMapping {
            base_lba: start_lba,
            allocated_blocks: needed_blocks,
            size_bytes,
            block_size,
        };
        inner.next_lba = end_lba;
        inner.map.insert(blob_id, mapping);
        Ok(mapping)
    }

    #[cfg(test)]
    pub fn reset_all(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.next_lba = 0;
    }
}

fn blocks_for(size_bytes: u64, block_size: u64) -> ExofsResult<u64> {
    if block_size == 0 {
        return Err(ExofsError::InvalidSize);
    }
    if size_bytes == 0 {
        return Ok(0);
    }
    size_bytes
        .checked_add(block_size.saturating_sub(1))
        .map(|n| n / block_size)
        .ok_or(ExofsError::OffsetOverflow)
}

pub fn persisted_size(blob_id: &BlobId) -> Option<u64> {
    OBJECT_STORE.persisted_size(blob_id)
}

pub fn persist_blob_data_if_disk(blob_id: BlobId, data: &[u8], sync: bool) -> ExofsResult<bool> {
    if !virtio_adapter::has_global_disk() {
        return Ok(false);
    }

    virtio_adapter::with_global_disk(|device| {
        let block_size = device.block_size();
        let block_size_usize = block_size as usize;
        if block_size_usize == 0 {
            return Err(ExofsError::InvalidSize);
        }

        let mapping = OBJECT_STORE.reserve_for_write(
            blob_id,
            data.len() as u64,
            block_size,
            device.total_blocks(),
        )?;

        let mut lba = mapping.base_lba;
        let mut pos = 0usize;
        while pos < data.len() {
            let chunk = core::cmp::min(block_size_usize, data.len().saturating_sub(pos));
            let mut block = alloc::vec![0u8; block_size_usize];
            block[..chunk].copy_from_slice(&data[pos..pos + chunk]);
            device.write_block(lba, &block)?;
            lba = lba.saturating_add(1);
            pos = pos.saturating_add(chunk);
        }

        if sync || data.is_empty() {
            device.flush().map_err(|_| ExofsError::NvmeFlushFailed)?;
        }

        Ok(true)
    })
}

pub fn load_blob_data_if_available(blob_id: &BlobId) -> ExofsResult<Option<Vec<u8>>> {
    let mapping = match OBJECT_STORE.lookup(blob_id) {
        Some(mapping) => mapping,
        None => return Ok(None),
    };

    if mapping.size_bytes == 0 {
        return Ok(Some(Vec::new()));
    }
    if !virtio_adapter::has_global_disk() {
        return Ok(None);
    }

    virtio_adapter::with_global_disk(|device| {
        if device.block_size() != mapping.block_size {
            return Err(ExofsError::InvalidState);
        }

        let mut out = Vec::new();
        out.try_reserve(mapping.size_bytes as usize)
            .map_err(|_| ExofsError::NoMemory)?;

        let block_size = mapping.block_size as usize;
        let mut remaining = mapping.size_bytes as usize;
        let mut lba = mapping.base_lba;
        while remaining > 0 {
            let mut block = alloc::vec![0u8; block_size];
            device.read_block(lba, &mut block)?;
            let take = core::cmp::min(remaining, block_size);
            out.extend_from_slice(&block[..take]);
            lba = lba.saturating_add(1);
            remaining = remaining.saturating_sub(take);
        }

        Ok(Some(out))
    })
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use spin::Mutex;

    struct MockBlockDevice {
        storage: Mutex<Vec<u8>>,
        block_size: u32,
    }

    impl MockBlockDevice {
        fn new(block_size: u32, blocks: usize) -> Self {
            Self {
                storage: Mutex::new(vec![0u8; block_size as usize * blocks]),
                block_size,
            }
        }
    }

    impl BlockDevice for MockBlockDevice {
        fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
            let block_size = self.block_size as usize;
            if buf.len() != block_size {
                return Err(ExofsError::InvalidArgument);
            }
            let start = lba as usize * block_size;
            let end = start.saturating_add(block_size);
            let storage = self.storage.lock();
            if end > storage.len() {
                return Err(ExofsError::IoError);
            }
            buf.copy_from_slice(&storage[start..end]);
            Ok(())
        }

        fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
            let block_size = self.block_size as usize;
            if buf.len() != block_size {
                return Err(ExofsError::InvalidArgument);
            }
            let start = lba as usize * block_size;
            let end = start.saturating_add(block_size);
            let mut storage = self.storage.lock();
            if end > storage.len() {
                return Err(ExofsError::IoError);
            }
            storage[start..end].copy_from_slice(buf);
            Ok(())
        }

        fn block_size(&self) -> u32 {
            self.block_size
        }

        fn total_blocks(&self) -> u64 {
            self.storage.lock().len() as u64 / self.block_size as u64
        }

        fn flush(&self) -> ExofsResult<()> {
            Ok(())
        }
    }

    #[test]
    fn persist_then_reload_roundtrip() {
        let device: Arc<dyn BlockDevice> = Arc::new(MockBlockDevice::new(512, 256));
        virtio_adapter::set_global_disk_for_test(device);
        OBJECT_STORE.reset_all();

        let blob_id = BlobId([0x5A; 32]);
        let payload = b"persisted object payload for exofs".to_vec();

        assert!(persist_blob_data_if_disk(blob_id, &payload, true).test_unwrap());
        let reloaded = load_blob_data_if_available(&blob_id).test_unwrap();
        assert_eq!(reloaded.test_unwrap(), payload);

        virtio_adapter::clear_global_disk_for_test();
        OBJECT_STORE.reset_all();
    }

    #[test]
    fn missing_disk_skips_persist() {
        virtio_adapter::clear_global_disk_for_test();
        OBJECT_STORE.reset_all();

        let blob_id = BlobId([0x1C; 32]);
        let wrote = persist_blob_data_if_disk(blob_id, b"cache-only", true).test_unwrap();
        assert!(!wrote);
        assert!(load_blob_data_if_available(&blob_id)
            .test_unwrap()
            .is_none());
    }
}
