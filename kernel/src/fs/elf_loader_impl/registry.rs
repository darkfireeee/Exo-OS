//! registry.rs — Registre des blobs ELF et métadonnées de segments

use crate::fs::exofs::core::types::BlobId;
use crate::memory::core::AllocError;
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::page_table::walker::FrameAllocatorForWalk;
use crate::memory::{AllocFlags, Frame};
use crate::process::lifecycle::exec::ElfLoadError;
use spin::Mutex;

pub const DYNAMIC_LOADER_PATH_MAX: usize = 128;
pub const ELF_BLOB_REGISTRY_CAP: usize = 1024;
pub const ELF_MAX_SEGMENTS_PER_BLOB: usize = 8;

#[derive(Clone, Copy)]
pub struct ElfSegmentMeta {
    pub file_data_start: u64,
    pub file_data_end: u64,
}

impl ElfSegmentMeta {
    pub const fn empty() -> Self {
        Self {
            file_data_start: 0,
            file_data_end: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DynamicLoaderHandoff {
    pub magic: u64,
    pub version: u32,
    pub flags: u32,
    pub executable_base: u64,
    pub executable_entry: u64,
    pub executable_phdr: u64,
    pub executable_phnum: u64,
    pub executable_phent: u64,
    pub executable_dynamic: u64,
    pub executable_dynamic_count: u64,
    pub interpreter_base: u64,
    pub interpreter_entry: u64,
    pub page_size: u64,
    pub executable_stack_top: u64,
    pub executable_path_len: u32,
    pub interpreter_path_len: u32,
    pub executable_path: [u8; DYNAMIC_LOADER_PATH_MAX],
    pub interpreter_path: [u8; DYNAMIC_LOADER_PATH_MAX],
}

#[derive(Clone, Copy)]
pub struct LoadedElfImage {
    pub entry_point: u64,
    pub base: u64,
    pub phdr_vaddr: u64,
    pub phnum: u64,
    pub phent: u64,
    pub dynamic_vaddr: u64,
    pub dynamic_count: u64,
    pub brk_end: u64,
}

#[derive(Clone, Copy)]
pub struct InterpreterPath {
    pub bytes: [u8; DYNAMIC_LOADER_PATH_MAX],
    pub len: usize,
}

impl InterpreterPath {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ElfLoadError> {
        if bytes.is_empty() || bytes.len() >= DYNAMIC_LOADER_PATH_MAX {
            return Err(ElfLoadError::InvalidElf);
        }
        let mut out = [0u8; DYNAMIC_LOADER_PATH_MAX];
        out[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: out,
            len: bytes.len(),
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub struct ElfBlobRegistry {
    ids: [BlobId; ELF_BLOB_REGISTRY_CAP],
    segments: [[ElfSegmentMeta; ELF_MAX_SEGMENTS_PER_BLOB]; ELF_BLOB_REGISTRY_CAP],
    segment_counts: [u8; ELF_BLOB_REGISTRY_CAP],
    count: usize,
}

impl ElfBlobRegistry {
    pub const fn new() -> Self {
        Self {
            ids: [BlobId::ZERO; ELF_BLOB_REGISTRY_CAP],
            segments: [[ElfSegmentMeta::empty(); ELF_MAX_SEGMENTS_PER_BLOB]; ELF_BLOB_REGISTRY_CAP],
            segment_counts: [0; ELF_BLOB_REGISTRY_CAP],
            count: 0,
        }
    }

    pub fn intern(&mut self, blob_id: BlobId) -> Option<u64> {
        for idx in 0..self.count {
            if self.ids[idx] == blob_id {
                return Some((idx + 1) as u64);
            }
        }
        if self.count >= ELF_BLOB_REGISTRY_CAP {
            return None;
        }
        let idx = self.count;
        self.ids[idx] = blob_id;
        self.segment_counts[idx] = 0;
        self.count += 1;
        Some((idx + 1) as u64)
    }

    pub fn get(&self, file_id: u64) -> Option<BlobId> {
        let idx = file_id.checked_sub(1)? as usize;
        if idx >= self.count {
            return None;
        }
        Some(self.ids[idx])
    }

    pub fn clear_segments(&mut self, file_id: u64) {
        let Some(idx) = file_id.checked_sub(1).map(|v| v as usize) else {
            return;
        };
        if idx >= self.count {
            return;
        }
        self.segment_counts[idx] = 0;
    }

    pub fn add_segment(&mut self, file_id: u64, p_offset: u64, p_filesz: u64) -> bool {
        let Some(idx) = file_id.checked_sub(1).map(|v| v as usize) else {
            return false;
        };
        if idx >= self.count {
            return false;
        }
        let slot = self.segment_counts[idx] as usize;
        if slot >= ELF_MAX_SEGMENTS_PER_BLOB {
            return false;
        }
        self.segments[idx][slot] = ElfSegmentMeta {
            file_data_start: p_offset,
            file_data_end: p_offset.saturating_add(p_filesz),
        };
        self.segment_counts[idx] = self.segment_counts[idx].saturating_add(1);
        true
    }

    pub fn segments_for(&self, file_id: u64) -> ([ElfSegmentMeta; ELF_MAX_SEGMENTS_PER_BLOB], usize) {
        let Some(idx) = file_id.checked_sub(1).map(|v| v as usize) else {
            return ([ElfSegmentMeta::empty(); ELF_MAX_SEGMENTS_PER_BLOB], 0);
        };
        if idx >= self.count {
            return ([ElfSegmentMeta::empty(); ELF_MAX_SEGMENTS_PER_BLOB], 0);
        }
        (
            self.segments[idx],
            (self.segment_counts[idx] as usize).min(ELF_MAX_SEGMENTS_PER_BLOB),
        )
    }
}

static ELF_BLOB_REGISTRY: Mutex<ElfBlobRegistry> = Mutex::new(ElfBlobRegistry::new());

pub fn register_elf_blob(blob_id: BlobId) -> Option<u64> {
    ELF_BLOB_REGISTRY.lock().intern(blob_id)
}

pub fn lookup_elf_blob(file_id: u64) -> Option<BlobId> {
    ELF_BLOB_REGISTRY.lock().get(file_id)
}

pub fn clear_elf_segments(file_id: u64) {
    ELF_BLOB_REGISTRY.lock().clear_segments(file_id);
}

pub fn register_elf_segment(file_id: u64, p_offset: u64, p_filesz: u64) -> Result<(), ElfLoadError> {
    if ELF_BLOB_REGISTRY
        .lock()
        .add_segment(file_id, p_offset, p_filesz)
    {
        Ok(())
    } else {
        Err(ElfLoadError::OutOfMemory)
    }
}

pub fn lookup_elf_segments(file_id: u64) -> ([ElfSegmentMeta; ELF_MAX_SEGMENTS_PER_BLOB], usize) {
    ELF_BLOB_REGISTRY.lock().segments_for(file_id)
}

pub struct ElfWalkAllocator;

impl FrameAllocatorForWalk for ElfWalkAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        buddy::alloc_pages(0, flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = buddy::free_pages(frame, 0);
    }
}
