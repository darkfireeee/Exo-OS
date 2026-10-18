//! parser.rs — Validation des en-têtes ELF, décodage des entiers et des Program Headers

use crate::arch::constants::USER_ELF_BASE_MIN;
use crate::memory::core::layout::USER_END;
use crate::memory::core::PageFlags;
use crate::memory::virt::vma::VmaFlags;
use crate::memory::PAGE_SIZE;
use crate::process::lifecycle::exec::ElfLoadError;

use super::registry::{InterpreterPath, DYNAMIC_LOADER_PATH_MAX};

pub const USER_ELF_BASE_MAX: u64 = USER_END.as_u64();
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;
pub const PF_X: u32 = 0x1;
pub const PF_W: u32 = 0x2;
pub const PF_R: u32 = 0x4;
pub const PHDR_SIZE: usize = 56;
pub const DEFAULT_DYNAMIC_LOADER_PATH: &[u8] = b"/lib/ld-exo.so";

const _: () = assert!(USER_ELF_BASE_MIN < USER_ELF_BASE_MAX);
const _: () = assert!(USER_ELF_BASE_MIN == 0x0040_0000);

pub fn validate_elf_header(data: &[u8]) -> Result<(), ElfLoadError> {
    if data.len() < 64 {
        return Err(ElfLoadError::InvalidElf);
    }
    if &data[0..4] != b"\x7FELF" {
        return Err(ElfLoadError::InvalidElf);
    }
    // EI_CLASS = 2 → 64-bit
    if data[4] != 2 {
        return Err(ElfLoadError::UnsupportedArch);
    }
    // EI_DATA = 1 → little-endian
    if data[5] != 1 {
        return Err(ElfLoadError::UnsupportedArch);
    }
    // e_machine = 0x3E → EM_X86_64
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    if e_machine != 0x3E {
        return Err(ElfLoadError::UnsupportedArch);
    }
    Ok(())
}

#[inline]
pub fn elf_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}

#[inline]
pub fn elf_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

#[inline]
pub fn elf_u64(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
}

pub fn phdr_span(data: &[u8]) -> Result<(usize, usize, usize), ElfLoadError> {
    if data.len() < 64 {
        return Err(ElfLoadError::InvalidElf);
    }
    let e_phoff = elf_u64(data, 32) as usize;
    let e_phentsize = elf_u16(data, 54) as usize;
    let e_phnum = elf_u16(data, 56) as usize;
    if e_phentsize < PHDR_SIZE {
        return Err(ElfLoadError::InvalidElf);
    }
    let table_bytes = e_phentsize
        .checked_mul(e_phnum)
        .ok_or(ElfLoadError::InvalidElf)?;
    let end = e_phoff
        .checked_add(table_bytes)
        .ok_or(ElfLoadError::InvalidElf)?;
    if end > data.len() {
        return Err(ElfLoadError::InvalidElf);
    }
    Ok((e_phoff, e_phnum, e_phentsize))
}

pub fn phdr_at(
    data: &[u8],
    e_phoff: usize,
    e_phentsize: usize,
    idx: usize,
) -> Result<&[u8], ElfLoadError> {
    let off = e_phoff
        .checked_add(idx.saturating_mul(e_phentsize))
        .ok_or(ElfLoadError::InvalidElf)?;
    let end = off.checked_add(PHDR_SIZE).ok_or(ElfLoadError::InvalidElf)?;
    if end > data.len() {
        return Err(ElfLoadError::InvalidElf);
    }
    Ok(&data[off..end])
}

pub fn read_interpreter_path(data: &[u8]) -> Result<Option<InterpreterPath>, ElfLoadError> {
    let (e_phoff, e_phnum, e_phentsize) = phdr_span(data)?;
    let mut i = 0usize;
    while i < e_phnum {
        let ph = phdr_at(data, e_phoff, e_phentsize, i)?;
        if elf_u32(ph, 0) == PT_INTERP {
            let offset = elf_u64(ph, 8) as usize;
            let filesz = elf_u64(ph, 32) as usize;
            if filesz == 0 || filesz > DYNAMIC_LOADER_PATH_MAX {
                return Err(ElfLoadError::InvalidElf);
            }
            let end = offset.checked_add(filesz).ok_or(ElfLoadError::InvalidElf)?;
            if end > data.len() {
                return Err(ElfLoadError::InvalidElf);
            }
            let raw = &data[offset..end];
            let len = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            if len == 0 || len >= DYNAMIC_LOADER_PATH_MAX {
                return Err(ElfLoadError::InvalidElf);
            }
            let mut bytes = [0u8; DYNAMIC_LOADER_PATH_MAX];
            bytes[..len].copy_from_slice(&raw[..len]);
            return Ok(Some(InterpreterPath { bytes, len }));
        }
        i += 1;
    }
    Ok(None)
}

pub fn default_interpreter_path(path: &str) -> Result<Option<InterpreterPath>, ElfLoadError> {
    if path.as_bytes() == DEFAULT_DYNAMIC_LOADER_PATH {
        return Ok(None);
    }
    InterpreterPath::from_bytes(DEFAULT_DYNAMIC_LOADER_PATH).map(Some)
}

pub fn validate_user_segment_range(p_vaddr: u64, p_memsz: usize) -> Result<(), ElfLoadError> {
    let Some(end_unaligned) = p_vaddr.checked_add(p_memsz as u64) else {
        return Err(ElfLoadError::InvalidElf);
    };
    let page_mask = PAGE_SIZE as u64 - 1;
    let start = p_vaddr & !page_mask;
    let end = end_unaligned.saturating_add(page_mask) & !page_mask;

    if start < USER_ELF_BASE_MIN {
        return Err(ElfLoadError::InvalidElf);
    }

    if end > USER_ELF_BASE_MAX {
        return Err(ElfLoadError::InvalidElf);
    }

    Ok(())
}

pub fn validate_load_segment_flags(p_flags: u32) -> Result<(), ElfLoadError> {
    if (p_flags & (PF_W | PF_X)) == (PF_W | PF_X) {
        return Err(ElfLoadError::PermissionDenied);
    }
    Ok(())
}

pub fn elf_page_flags(p_flags: u32) -> PageFlags {
    debug_assert!(validate_load_segment_flags(p_flags).is_ok());

    let exec = (p_flags & PF_X) != 0;
    let write = (p_flags & PF_W) != 0;

    match (exec, write) {
        (true, true) => PageFlags::USER_DATA,
        (true, false) => PageFlags::USER_CODE,
        (false, true) => PageFlags::USER_DATA,
        (false, false) => PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE,
    }
}

pub fn elf_vma_flags(p_flags: u32) -> VmaFlags {
    debug_assert!(validate_load_segment_flags(p_flags).is_ok());

    let mut flags = VmaFlags::NONE;
    if (p_flags & PF_R) != 0 {
        flags |= VmaFlags::READ;
    }
    if (p_flags & PF_W) != 0 {
        flags |= VmaFlags::WRITE;
    }
    if (p_flags & PF_X) != 0 {
        flags |= VmaFlags::EXEC;
    }
    flags
}
