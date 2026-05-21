//! object_store.rs — Persistance simple des blobs de syscall vers block device.
//!
//! Cette couche garde un mapping `BlobId -> plage LBA` afin de permettre
//! la réouverture/relire depuis le disque même après purge du cache RAM.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use crate::fs::exofs::storage::virtio_adapter;
use crate::scheduler::sync::spinlock::SpinLock;

const DATA_LBA_START: u64 = 2048;
const RESERVED_LBA_END: u64 = 0x0301;
const OBJECT_INDEX_LBA: u64 = 64;
const OBJECT_INDEX_BLOCKS: u64 = 128;
const OBJECT_INDEX_MAGIC: u32 = 0x4558_4F49; // "EXOI"
const OBJECT_INDEX_VERSION: u16 = 1;
const OBJECT_INDEX_HEADER_SIZE: usize = 32;
const OBJECT_INDEX_ENTRY_SIZE: usize = 64;
const OBJECT_INDEX_FREE_ENTRY_SIZE: usize = 16;

static CATALOG_LOADED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedBlobMapping {
    pub base_lba: u64,
    pub allocated_blocks: u64,
    pub size_bytes: u64,
    pub block_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FreeExtent {
    base_lba: u64,
    blocks: u64,
}

struct ObjectStoreInner {
    map: BTreeMap<BlobId, PersistedBlobMapping>,
    free_extents: Vec<FreeExtent>,
    next_lba: u64,
}

impl ObjectStoreInner {
    const fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            free_extents: Vec::new(),
            next_lba: 0,
        }
    }

    fn reset(&mut self) {
        self.map.clear();
        self.free_extents.clear();
        self.next_lba = 0;
    }

    fn add_free_extent(&mut self, base_lba: u64, blocks: u64) -> ExofsResult<()> {
        if blocks == 0 {
            return Ok(());
        }
        self.free_extents
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.free_extents.push(FreeExtent { base_lba, blocks });
        self.merge_free_extents();
        Ok(())
    }

    fn merge_free_extents(&mut self) {
        self.free_extents.sort_by_key(|extent| extent.base_lba);
        let mut write = 0usize;
        let mut read = 0usize;
        while read < self.free_extents.len() {
            let mut cur = self.free_extents[read];
            read = read.wrapping_add(1);
            while read < self.free_extents.len() {
                let next = self.free_extents[read];
                let cur_end = cur.base_lba.saturating_add(cur.blocks);
                if cur_end < next.base_lba {
                    break;
                }
                let next_end = next.base_lba.saturating_add(next.blocks);
                cur.blocks = next_end.saturating_sub(cur.base_lba);
                read = read.wrapping_add(1);
            }
            self.free_extents[write] = cur;
            write = write.wrapping_add(1);
        }
        self.free_extents.truncate(write);
    }

    fn take_free_extent(&mut self, needed_blocks: u64) -> Option<u64> {
        if needed_blocks == 0 {
            return None;
        }
        let mut i = 0usize;
        while i < self.free_extents.len() {
            let extent = self.free_extents[i];
            if extent.blocks >= needed_blocks {
                let base = extent.base_lba;
                if extent.blocks == needed_blocks {
                    self.free_extents.remove(i);
                } else {
                    self.free_extents[i].base_lba =
                        self.free_extents[i].base_lba.saturating_add(needed_blocks);
                    self.free_extents[i].blocks =
                        self.free_extents[i].blocks.saturating_sub(needed_blocks);
                }
                return Some(base);
            }
            i = i.wrapping_add(1);
        }
        None
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

        let mut old_extent = None;
        if let Some(existing) = inner.map.get_mut(&blob_id) {
            if existing.block_size != block_size {
                return Err(ExofsError::InvalidArgument);
            }
            if existing.allocated_blocks >= needed_blocks {
                existing.size_bytes = size_bytes;
                return Ok(*existing);
            }
            old_extent = Some(*existing);
        }
        if let Some(old) = old_extent {
            inner.add_free_extent(old.base_lba, old.allocated_blocks)?;
        }

        let baseline_lba = data_lba_start(total_blocks);
        let start_lba = if needed_blocks == 0 {
            inner
                .map
                .get(&blob_id)
                .map(|mapping| mapping.base_lba)
                .unwrap_or(baseline_lba)
        } else if let Some(reused) = inner.take_free_extent(needed_blocks) {
            reused
        } else if inner.next_lba == 0 {
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
        inner.next_lba = inner.next_lba.max(end_lba);
        inner.map.insert(blob_id, mapping);
        Ok(mapping)
    }

    pub fn free_lba(&self, blob_id: &BlobId) -> ExofsResult<Option<PersistedBlobMapping>> {
        let mut inner = self.inner.lock();
        let Some(mapping) = inner.map.remove(blob_id) else {
            return Ok(None);
        };
        inner.add_free_extent(mapping.base_lba, mapping.allocated_blocks)?;
        Ok(Some(mapping))
    }

    fn snapshot_catalog(
        &self,
    ) -> ExofsResult<(Vec<(BlobId, PersistedBlobMapping)>, Vec<FreeExtent>, u64)> {
        let inner = self.inner.lock();
        let mut mappings = Vec::new();
        mappings
            .try_reserve(inner.map.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for (blob_id, mapping) in inner.map.iter() {
            mappings.push((*blob_id, *mapping));
        }
        let mut free_extents = Vec::new();
        free_extents
            .try_reserve(inner.free_extents.len())
            .map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < inner.free_extents.len() {
            free_extents.push(inner.free_extents[i]);
            i = i.wrapping_add(1);
        }
        Ok((mappings, free_extents, inner.next_lba))
    }

    fn replace_catalog(
        &self,
        mappings: Vec<(BlobId, PersistedBlobMapping)>,
        free_extents: Vec<FreeExtent>,
        next_lba: u64,
    ) {
        let mut inner = self.inner.lock();
        inner.reset();
        let mut computed_next = next_lba;
        let mut i = 0usize;
        while i < mappings.len() {
            let (blob_id, mapping) = mappings[i];
            computed_next =
                computed_next.max(mapping.base_lba.saturating_add(mapping.allocated_blocks));
            inner.map.insert(blob_id, mapping);
            i = i.wrapping_add(1);
        }
        inner.free_extents = free_extents;
        inner.merge_free_extents();
        inner.next_lba = computed_next;
    }

    #[cfg(test)]
    pub fn reset_all(&self) {
        let mut inner = self.inner.lock();
        inner.reset();
        CATALOG_LOADED.store(false, Ordering::Release);
    }
}

fn data_lba_start(total_blocks: u64) -> u64 {
    if total_blocks > DATA_LBA_START {
        DATA_LBA_START
    } else if total_blocks > RESERVED_LBA_END {
        RESERVED_LBA_END
    } else if total_blocks > OBJECT_INDEX_LBA.saturating_add(OBJECT_INDEX_BLOCKS) {
        OBJECT_INDEX_LBA.saturating_add(OBJECT_INDEX_BLOCKS)
    } else {
        1
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
    let _ = ensure_catalog_loaded();
    OBJECT_STORE.persisted_size(blob_id)
}

pub fn lookup_mapping(blob_id: &BlobId) -> Option<PersistedBlobMapping> {
    let _ = ensure_catalog_loaded();
    OBJECT_STORE.lookup(blob_id)
}

pub fn mapping_disk_offset(blob_id: &BlobId) -> Option<crate::fs::exofs::core::DiskOffset> {
    lookup_mapping(blob_id).map(|mapping| {
        crate::fs::exofs::core::DiskOffset(
            mapping.base_lba.saturating_mul(mapping.block_size as u64),
        )
    })
}

fn ensure_catalog_loaded() -> ExofsResult<()> {
    if CATALOG_LOADED.load(Ordering::Acquire) || !virtio_adapter::has_global_disk() {
        return Ok(());
    }
    load_catalog_from_global_disk().map(|_| ())
}

fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn read_u16(buf: &[u8], off: usize) -> ExofsResult<u16> {
    if off.saturating_add(2) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u16::from_le_bytes([buf[off], buf[off + 1]]))
}

fn read_u32(buf: &[u8], off: usize) -> ExofsResult<u32> {
    if off.saturating_add(4) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u32::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
    ]))
}

fn read_u64(buf: &[u8], off: usize) -> ExofsResult<u64> {
    if off.saturating_add(8) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ]))
}

fn serialize_catalog(
    mappings: &[(BlobId, PersistedBlobMapping)],
    free_extents: &[FreeExtent],
    next_lba: u64,
    block_size: usize,
) -> ExofsResult<Vec<u8>> {
    let total = block_size
        .checked_mul(OBJECT_INDEX_BLOCKS as usize)
        .ok_or(ExofsError::OffsetOverflow)?;
    let used = OBJECT_INDEX_HEADER_SIZE
        .saturating_add(mappings.len().saturating_mul(OBJECT_INDEX_ENTRY_SIZE))
        .saturating_add(
            free_extents
                .len()
                .saturating_mul(OBJECT_INDEX_FREE_ENTRY_SIZE),
        );
    if used > total {
        return Err(ExofsError::NoSpace);
    }
    let mut buf = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    push_u32(&mut buf, OBJECT_INDEX_MAGIC);
    push_u16(&mut buf, OBJECT_INDEX_VERSION);
    push_u16(&mut buf, 0);
    push_u64(&mut buf, next_lba);
    push_u32(&mut buf, mappings.len() as u32);
    push_u32(&mut buf, free_extents.len() as u32);
    push_u64(&mut buf, 0);

    let mut i = 0usize;
    while i < mappings.len() {
        let (blob_id, mapping) = mappings[i];
        buf.extend_from_slice(blob_id.as_bytes());
        push_u64(&mut buf, mapping.base_lba);
        push_u64(&mut buf, mapping.allocated_blocks);
        push_u64(&mut buf, mapping.size_bytes);
        push_u32(&mut buf, mapping.block_size);
        push_u32(&mut buf, 0);
        i = i.wrapping_add(1);
    }

    let mut j = 0usize;
    while j < free_extents.len() {
        push_u64(&mut buf, free_extents[j].base_lba);
        push_u64(&mut buf, free_extents[j].blocks);
        j = j.wrapping_add(1);
    }

    buf.resize(total, 0);
    Ok(buf)
}

fn deserialize_catalog(
    buf: &[u8],
) -> ExofsResult<Option<(Vec<(BlobId, PersistedBlobMapping)>, Vec<FreeExtent>, u64)>> {
    if buf.len() < OBJECT_INDEX_HEADER_SIZE {
        return Ok(None);
    }
    let magic = read_u32(buf, 0)?;
    if magic == 0 {
        return Ok(None);
    }
    if magic != OBJECT_INDEX_MAGIC {
        return Err(ExofsError::InvalidMagic);
    }
    let version = read_u16(buf, 4)?;
    if version != OBJECT_INDEX_VERSION {
        return Err(ExofsError::IncompatibleVersion);
    }
    let next_lba = read_u64(buf, 8)?;
    let mapping_count = read_u32(buf, 16)? as usize;
    let free_count = read_u32(buf, 20)? as usize;
    let needed = OBJECT_INDEX_HEADER_SIZE
        .saturating_add(mapping_count.saturating_mul(OBJECT_INDEX_ENTRY_SIZE))
        .saturating_add(free_count.saturating_mul(OBJECT_INDEX_FREE_ENTRY_SIZE));
    if needed > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }

    let mut mappings = Vec::new();
    mappings
        .try_reserve(mapping_count)
        .map_err(|_| ExofsError::NoMemory)?;
    let mut off = OBJECT_INDEX_HEADER_SIZE;
    let mut i = 0usize;
    while i < mapping_count {
        let mut id = [0u8; 32];
        id.copy_from_slice(&buf[off..off + 32]);
        off = off.saturating_add(32);
        let mapping = PersistedBlobMapping {
            base_lba: read_u64(buf, off)?,
            allocated_blocks: read_u64(buf, off + 8)?,
            size_bytes: read_u64(buf, off + 16)?,
            block_size: read_u32(buf, off + 24)?,
        };
        off = off.saturating_add(32);
        mappings.push((BlobId(id), mapping));
        i = i.wrapping_add(1);
    }

    let mut free_extents = Vec::new();
    free_extents
        .try_reserve(free_count)
        .map_err(|_| ExofsError::NoMemory)?;
    let mut j = 0usize;
    while j < free_count {
        free_extents.push(FreeExtent {
            base_lba: read_u64(buf, off)?,
            blocks: read_u64(buf, off + 8)?,
        });
        off = off.saturating_add(OBJECT_INDEX_FREE_ENTRY_SIZE);
        j = j.wrapping_add(1);
    }

    Ok(Some((mappings, free_extents, next_lba)))
}

pub fn persist_catalog_to_global_disk() -> ExofsResult<bool> {
    if !virtio_adapter::has_global_disk() {
        return Ok(false);
    }
    let (mappings, free_extents, next_lba) = OBJECT_STORE.snapshot_catalog()?;
    virtio_adapter::with_global_disk(|device| {
        let block_size = device.block_size() as usize;
        if block_size == 0 {
            return Err(ExofsError::InvalidSize);
        }
        let buf = serialize_catalog(&mappings, &free_extents, next_lba, block_size)?;
        let mut lba = OBJECT_INDEX_LBA;
        let mut pos = 0usize;
        while pos < buf.len() {
            device.write_block(lba, &buf[pos..pos + block_size])?;
            lba = lba.saturating_add(1);
            pos = pos.saturating_add(block_size);
        }
        device.flush().map_err(|_| ExofsError::NvmeFlushFailed)?;
        CATALOG_LOADED.store(true, Ordering::Release);
        Ok(true)
    })
}

pub fn load_catalog_from_global_disk() -> ExofsResult<bool> {
    if !virtio_adapter::has_global_disk() {
        return Ok(false);
    }
    virtio_adapter::with_global_disk(|device| {
        let block_size = device.block_size() as usize;
        if block_size == 0 {
            return Err(ExofsError::InvalidSize);
        }
        let total = block_size
            .checked_mul(OBJECT_INDEX_BLOCKS as usize)
            .ok_or(ExofsError::OffsetOverflow)?;
        let mut buf = Vec::new();
        buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        buf.resize(total, 0);
        let mut lba = OBJECT_INDEX_LBA;
        let mut pos = 0usize;
        while pos < total {
            device.read_block(lba, &mut buf[pos..pos + block_size])?;
            lba = lba.saturating_add(1);
            pos = pos.saturating_add(block_size);
        }
        if let Some((mappings, free_extents, next_lba)) = deserialize_catalog(&buf)? {
            OBJECT_STORE.replace_catalog(mappings, free_extents, next_lba);
        }
        CATALOG_LOADED.store(true, Ordering::Release);
        Ok(true)
    })
}

pub fn free_blob_mapping(blob_id: &BlobId) -> ExofsResult<Option<PersistedBlobMapping>> {
    let freed = OBJECT_STORE.free_lba(blob_id)?;
    if freed.is_some() {
        let _ = persist_catalog_to_global_disk()?;
    }
    Ok(freed)
}

pub fn persist_blob_data_if_disk(blob_id: BlobId, data: &[u8], sync: bool) -> ExofsResult<bool> {
    if !virtio_adapter::has_global_disk() {
        return Ok(false);
    }
    ensure_catalog_loaded()?;

    let wrote = virtio_adapter::with_global_disk(|device| {
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
        let mut block = Vec::new();
        block
            .try_reserve_exact(block_size_usize)
            .map_err(|_| ExofsError::NoMemory)?;
        block.resize(block_size_usize, 0);
        while pos < data.len() {
            let chunk = core::cmp::min(block_size_usize, data.len().saturating_sub(pos));
            block.fill(0);
            block[..chunk].copy_from_slice(&data[pos..pos + chunk]);
            device.write_block(lba, &block)?;
            lba = lba.saturating_add(1);
            pos = pos.saturating_add(chunk);
        }

        if sync || data.is_empty() {
            device.flush().map_err(|_| ExofsError::NvmeFlushFailed)?;
        }

        Ok(true)
    })?;
    if wrote {
        let _ = persist_catalog_to_global_disk()?;
    }
    Ok(wrote)
}

pub fn load_blob_data_if_available(blob_id: &BlobId) -> ExofsResult<Option<Vec<u8>>> {
    ensure_catalog_loaded()?;
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
        let mut block = Vec::new();
        block
            .try_reserve_exact(block_size)
            .map_err(|_| ExofsError::NoMemory)?;
        block.resize(block_size, 0);
        while remaining > 0 {
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
    use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
    use alloc::sync::Arc;
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

        OBJECT_STORE.reset_all();
        assert!(load_catalog_from_global_disk().test_unwrap());
        let reloaded_after_catalog = load_blob_data_if_available(&blob_id).test_unwrap();
        assert_eq!(reloaded_after_catalog.test_unwrap(), payload);

        virtio_adapter::clear_global_disk_for_test();
        OBJECT_STORE.reset_all();
    }

    #[test]
    fn freed_extent_is_reused() {
        let device: Arc<dyn BlockDevice> = Arc::new(MockBlockDevice::new(512, 1024));
        virtio_adapter::set_global_disk_for_test(device);
        OBJECT_STORE.reset_all();

        let first = BlobId([0x21; 32]);
        let second = BlobId([0x22; 32]);
        persist_blob_data_if_disk(first, b"first payload", true).test_unwrap();
        let first_mapping = lookup_mapping(&first).test_unwrap();
        free_blob_mapping(&first).test_unwrap();
        persist_blob_data_if_disk(second, b"second", true).test_unwrap();
        let second_mapping = lookup_mapping(&second).test_unwrap();

        assert_eq!(first_mapping.base_lba, second_mapping.base_lba);

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
