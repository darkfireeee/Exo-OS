//! ELF Loader Module
//! 
//! Loads ELF64 executables and shared libraries into process address space.
//! Supports:
//! - Static executables (ET_EXEC)
//! - Position Independent Executables (ET_DYN)
//! - Program headers (PT_LOAD, PT_INTERP, PT_TLS)
//! - Relocations (R_X86_64_*)

pub mod elf64;
pub mod process_image;
pub mod spawn;

pub use elf64::*;
pub use process_image::*;
pub use spawn::*;

use crate::memory::{MemoryError, VirtualAddress, PAGE_SIZE};

/// ELF loading errors
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ElfError {
    /// Invalid ELF magic number
    InvalidMagic,
    /// Not a 64-bit ELF
    Not64Bit,
    /// Not little endian
    NotLittleEndian,
    /// Invalid ELF type (not EXEC or DYN)
    InvalidType,
    /// Not x86_64 architecture
    WrongArchitecture,
    /// Program header table missing
    NoProgramHeaders,
    /// Section out of bounds
    SectionOutOfBounds,
    /// String table missing
    NoStringTable,
    /// Invalid program header
    InvalidProgramHeader,
    /// Memory mapping failed
    MappingFailed(MemoryError),
    /// Relocation failed
    RelocationFailed,
    /// TLS too large
    TlsTooLarge,
    /// Buffer too small
    BufferTooSmall,
    /// Missing interpreter
    MissingInterpreter,
}

/// Result type for ELF operations
pub type ElfResult<T> = Result<T, ElfError>;

/// Load an ELF executable from a byte buffer
/// 
/// Returns the entry point address on success
pub fn load_elf(elf_data: &[u8], base_address: Option<VirtualAddress>) -> ElfResult<LoadedElf> {
    let header = Elf64Header::parse(elf_data)?;
    
    // Validate it's an executable or shared object
    if header.e_type != ElfType::Exec as u16 && header.e_type != ElfType::Dyn as u16 {
        return Err(ElfError::InvalidType);
    }
    
    // Calculate load bias for PIE
    let load_bias = if header.e_type == ElfType::Dyn as u16 {
        base_address.unwrap_or(VirtualAddress::new(0x400000))
    } else {
        VirtualAddress::new(0)
    };
    
    let mut loaded = LoadedElf {
        entry_point: VirtualAddress::new((header.e_entry as usize).wrapping_add(load_bias.as_usize())),
        load_bias,
        segments: alloc::vec::Vec::new(),
        tls_template: None,
        interpreter: None,
        phdr_addr: VirtualAddress::new(0),
        phdr_num: header.e_phnum as usize,
        phdr_size: header.e_phentsize as usize,
    };
    
    // Process program headers
    let ph_offset = header.e_phoff as usize;
    let ph_size = header.e_phentsize as usize;
    let ph_num = header.e_phnum as usize;
    
    if ph_offset + ph_size * ph_num > elf_data.len() {
        return Err(ElfError::SectionOutOfBounds);
    }
    
    for i in 0..ph_num {
        let ph_data = &elf_data[ph_offset + i * ph_size..ph_offset + (i + 1) * ph_size];
        let phdr = Elf64ProgramHeader::parse(ph_data)?;
        
        match phdr.p_type {
            PT_LOAD => {
                let segment = process_load_segment(&phdr, elf_data, load_bias)?;
                loaded.segments.push(segment);
            }
            PT_INTERP => {
                // Dynamic linker path
                let interp_start = phdr.p_offset as usize;
                let interp_end = interp_start + phdr.p_filesz as usize;
                if interp_end > elf_data.len() {
                    return Err(ElfError::SectionOutOfBounds);
                }
                let interp_bytes = &elf_data[interp_start..interp_end];
                // Remove null terminator if present
                let interp_str = core::str::from_utf8(
                    interp_bytes.strip_suffix(&[0]).unwrap_or(interp_bytes)
                ).unwrap_or("/lib/ld-linux-x86-64.so.2");
                loaded.interpreter = Some(alloc::string::String::from(interp_str));
            }
            PT_TLS => {
                loaded.tls_template = Some(TlsTemplate {
                    addr: VirtualAddress::new((phdr.p_vaddr as usize).wrapping_add(load_bias.as_usize())),
                    file_size: phdr.p_filesz as usize,
                    mem_size: phdr.p_memsz as usize,
                    align: phdr.p_align as usize,
                });
            }
            PT_PHDR => {
                loaded.phdr_addr = VirtualAddress::new(
                    (phdr.p_vaddr as usize).wrapping_add(load_bias.as_usize())
                );
            }
            _ => {} // Ignore other types
        }
    }
    
    Ok(loaded)
}

/// Process a PT_LOAD segment
fn process_load_segment(
    phdr: &Elf64ProgramHeader,
    elf_data: &[u8],
    load_bias: VirtualAddress,
) -> ElfResult<LoadedSegment> {
    let vaddr = (phdr.p_vaddr as usize).wrapping_add(load_bias.as_usize());
    let file_offset = phdr.p_offset as usize;
    let file_size = phdr.p_filesz as usize;
    let mem_size = phdr.p_memsz as usize;
    
    // Validate bounds
    if file_offset + file_size > elf_data.len() {
        return Err(ElfError::SectionOutOfBounds);
    }
    
    // Calculate page-aligned addresses
    let page_offset = vaddr & (PAGE_SIZE - 1);
    let aligned_vaddr = vaddr & !(PAGE_SIZE - 1);
    let aligned_size = (mem_size + page_offset + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    
    // Convert flags
    let flags = SegmentFlags {
        read: phdr.p_flags & PF_R != 0,
        write: phdr.p_flags & PF_W != 0,
        execute: phdr.p_flags & PF_X != 0,
    };
    
    Ok(LoadedSegment {
        vaddr: VirtualAddress::new(aligned_vaddr),
        mem_size: aligned_size,
        file_offset,
        file_size,
        page_offset,
        flags,
        data_offset: file_offset,
    })
}

// Re-export extern alloc for use with no_std
extern crate alloc;
