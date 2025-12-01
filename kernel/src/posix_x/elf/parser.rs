//! ELF64 Binary Parser - Phase 10
//!
//! Parses ELF64 headers and program headers for execve()

use core::mem;

/// ELF Magic number
pub const ELF_MAGIC: &[u8; 4] = b"\x7FELF";

/// ELF Class - 64-bit
pub const ELFCLASS64: u8 = 2;

/// ELF Data encoding - Little endian
pub const ELFDATA2LSB: u8 = 1;

/// ELF Version
pub const EV_CURRENT: u8 = 1;

/// Program header types
pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1; // Loadable segment
pub const PT_DYNAMIC: u32 = 2; // Dynamic linking info
pub const PT_INTERP: u32 = 3; // Interpreter path
pub const PT_NOTE: u32 = 4;
pub const PT_SHLIB: u32 = 5;
pub const PT_PHDR: u32 = 6;

/// Program header flags
pub const PF_X: u32 = 0x1; // Executable
pub const PF_W: u32 = 0x2; // Writable
pub const PF_R: u32 = 0x4; // Readable

/// ELF64 File Header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    pub e_ident: [u8; 16], // Magic + metadata
    pub e_type: u16,       // Object file type
    pub e_machine: u16,    // Architecture
    pub e_version: u32,    // Version
    pub e_entry: u64,      // Entry point address
    pub e_phoff: u64,      // Program header offset
    pub e_shoff: u64,      // Section header offset
    pub e_flags: u32,      // Processor flags
    pub e_ehsize: u16,     // ELF header size
    pub e_phentsize: u16,  // Program header entry size
    pub e_phnum: u16,      // Number of program headers
    pub e_shentsize: u16,  // Section header entry size
    pub e_shnum: u16,      // Number of section headers
    pub e_shstrndx: u16,   // String table index
}

/// ELF64 Program Header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,   // Segment type
    pub p_flags: u32,  // Segment flags
    pub p_offset: u64, // File offset
    pub p_vaddr: u64,  // Virtual address
    pub p_paddr: u64,  // Physical address (unused)
    pub p_filesz: u64, // Size in file
    pub p_memsz: u64,  // Size in memory
    pub p_align: u64,  // Alignment
}

/// ELF parsing errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    BadVersion,
    BadProgramHeader,
    InvalidHeader,
}

/// Parse ELF64 header from bytes
pub fn parse_elf_header(data: &[u8]) -> Result<&Elf64Header, ElfError> {
    // Check minimum size
    if data.len() < mem::size_of::<Elf64Header>() {
        return Err(ElfError::TooSmall);
    }

    let header = unsafe { &*(data.as_ptr() as *const Elf64Header) };

    // Verify magic number
    if &header.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }

    // Verify 64-bit
    if header.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::Not64Bit);
    }

    // Verify little-endian
    if header.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }

    // Verify version
    if header.e_ident[6] != EV_CURRENT {
        return Err(ElfError::BadVersion);
    }

    // Verify header size
    if header.e_ehsize as usize != mem::size_of::<Elf64Header>() {
        return Err(ElfError::InvalidHeader);
    }

    Ok(header)
}

/// Get program headers slice from ELF data
pub fn get_program_headers<'a>(
    data: &'a [u8],
    header: &Elf64Header,
) -> Result<&'a [Elf64ProgramHeader], ElfError> {
    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    // Verify program header size
    if ph_size != mem::size_of::<Elf64ProgramHeader>() {
        return Err(ElfError::BadProgramHeader);
    }

    // Check bounds
    let ph_end = ph_offset
        .checked_add(ph_count.checked_mul(ph_size).ok_or(ElfError::TooSmall)?)
        .ok_or(ElfError::TooSmall)?;

    if ph_end > data.len() {
        return Err(ElfError::TooSmall);
    }

    // Create slice
    let slice = unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(ph_offset) as *const Elf64ProgramHeader,
            ph_count,
        )
    };

    Ok(slice)
}

/// Get segment data for a program header
pub fn get_segment_data<'a>(data: &'a [u8], ph: &Elf64ProgramHeader) -> Result<&'a [u8], ElfError> {
    let offset = ph.p_offset as usize;
    let size = ph.p_filesz as usize;

    // Check bounds
    let end = offset.checked_add(size).ok_or(ElfError::TooSmall)?;

    if end > data.len() {
        return Err(ElfError::TooSmall);
    }

    Ok(&data[offset..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elf_magic() {
        assert_eq!(ELF_MAGIC, b"\x7FELF");
    }
}
