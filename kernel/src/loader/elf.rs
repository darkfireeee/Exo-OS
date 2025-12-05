//! ELF Loader - Execute ELF binaries
//!
//! Phase 4C: exec() implementation
//! Parses and loads ELF (Executable and Linkable Format) files

use core::mem;

/// ELF magic number
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF class
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfClass {
    None = 0,
    Elf32 = 1,
    Elf64 = 2,
}

/// ELF data encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfData {
    None = 0,
    Little = 1,  // Little-endian
    Big = 2,     // Big-endian
}

/// ELF type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfType {
    None = 0,
    Rel = 1,      // Relocatable
    Exec = 2,     // Executable
    Dyn = 3,      // Shared object
    Core = 4,     // Core file
}

/// Program header type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgType {
    Null = 0,
    Load = 1,       // Loadable segment
    Dynamic = 2,    // Dynamic linking info
    Interp = 3,     // Interpreter path
    Note = 4,       // Auxiliary info
    Shlib = 5,      // Reserved
    Phdr = 6,       // Program header table
    Tls = 7,        // Thread-local storage
}

/// Program header flags
#[derive(Debug, Clone, Copy)]
pub struct ProgFlags(pub u32);

impl ProgFlags {
    pub const EXECUTE: u32 = 0x1;
    pub const WRITE: u32 = 0x2;
    pub const READ: u32 = 0x4;
    
    pub fn is_executable(&self) -> bool { self.0 & Self::EXECUTE != 0 }
    pub fn is_writable(&self) -> bool { self.0 & Self::WRITE != 0 }
    pub fn is_readable(&self) -> bool { self.0 & Self::READ != 0 }
}

/// ELF64 header (52 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    pub ident: [u8; 16],        // Magic + class + data + version + ABI
    pub typ: u16,               // Object file type
    pub machine: u16,           // Architecture
    pub version: u32,           // ELF version
    pub entry: u64,             // Entry point virtual address
    pub phoff: u64,             // Program header table offset
    pub shoff: u64,             // Section header table offset
    pub flags: u32,             // Processor-specific flags
    pub ehsize: u16,            // ELF header size
    pub phentsize: u16,         // Program header entry size
    pub phnum: u16,             // Program header entry count
    pub shentsize: u16,         // Section header entry size
    pub shnum: u16,             // Section header entry count
    pub shstrndx: u16,          // Section name string table index
}

/// ELF64 program header (56 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    pub typ: u32,               // Segment type
    pub flags: u32,             // Segment flags
    pub offset: u64,            // Segment file offset
    pub vaddr: u64,             // Segment virtual address
    pub paddr: u64,             // Segment physical address (unused)
    pub filesz: u64,            // Segment size in file
    pub memsz: u64,             // Segment size in memory
    pub align: u64,             // Segment alignment
}

/// ELF64 section header (64 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64SectionHeader {
    pub name: u32,              // Section name (string table index)
    pub typ: u32,               // Section type
    pub flags: u64,             // Section flags
    pub addr: u64,              // Section virtual address
    pub offset: u64,            // Section file offset
    pub size: u64,              // Section size in bytes
    pub link: u32,              // Link to another section
    pub info: u32,              // Additional section info
    pub addralign: u64,         // Section alignment
    pub entsize: u64,           // Entry size for sections with fixed-size entries
}

/// ELF parsing error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    /// Invalid ELF magic number
    InvalidMagic,
    /// Unsupported ELF class (not 64-bit)
    UnsupportedClass,
    /// Unsupported endianness (not little-endian)
    UnsupportedEndian,
    /// Unsupported architecture (not x86-64)
    UnsupportedArch,
    /// Invalid ELF type (not executable or shared object)
    InvalidType,
    /// File too small
    FileTooSmall,
    /// Invalid program header
    InvalidProgramHeader,
    /// No loadable segments found
    NoLoadableSegments,
    /// Invalid entry point
    InvalidEntryPoint,
}

pub type ElfResult<T> = Result<T, ElfError>;

/// Parsed ELF file
pub struct ElfFile<'a> {
    data: &'a [u8],
    header: &'a Elf64Header,
}

impl<'a> ElfFile<'a> {
    /// Parse ELF file from bytes
    pub fn parse(data: &'a [u8]) -> ElfResult<Self> {
        // Check minimum size
        if data.len() < mem::size_of::<Elf64Header>() {
            return Err(ElfError::FileTooSmall);
        }
        
        // Parse header
        let header = unsafe { &*(data.as_ptr() as *const Elf64Header) };
        
        // Validate magic
        if header.ident[0..4] != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }
        
        // Check class (64-bit)
        if header.ident[4] != ElfClass::Elf64 as u8 {
            return Err(ElfError::UnsupportedClass);
        }
        
        // Check endianness (little-endian)
        if header.ident[5] != ElfData::Little as u8 {
            return Err(ElfError::UnsupportedEndian);
        }
        
        // Check architecture (x86-64 = 0x3E)
        if header.machine != 0x3E {
            return Err(ElfError::UnsupportedArch);
        }
        
        // Check type (executable or shared object)
        if header.typ != ElfType::Exec as u16 && header.typ != ElfType::Dyn as u16 {
            return Err(ElfError::InvalidType);
        }
        
        Ok(Self { data, header })
    }
    
    /// Get ELF header
    pub fn header(&self) -> &Elf64Header {
        self.header
    }
    
    /// Get entry point address
    pub fn entry_point(&self) -> u64 {
        self.header.entry
    }
    
    /// Get program headers
    pub fn program_headers(&self) -> ElfResult<&'a [Elf64ProgramHeader]> {
        let offset = self.header.phoff as usize;
        let count = self.header.phnum as usize;
        let entry_size = self.header.phentsize as usize;
        
        if offset + (count * entry_size) > self.data.len() {
            return Err(ElfError::InvalidProgramHeader);
        }
        
        let slice = &self.data[offset..offset + (count * entry_size)];
        let headers = unsafe {
            core::slice::from_raw_parts(
                slice.as_ptr() as *const Elf64ProgramHeader,
                count
            )
        };
        
        Ok(headers)
    }
    
    /// Get loadable segments (PT_LOAD)
    pub fn loadable_segments(&self) -> ElfResult<impl Iterator<Item = &Elf64ProgramHeader>> {
        let headers = self.program_headers()?;
        Ok(headers.iter().filter(|h| h.typ == ProgType::Load as u32))
    }
    
    /// Get segment data
    pub fn segment_data(&self, header: &Elf64ProgramHeader) -> ElfResult<&'a [u8]> {
        let offset = header.offset as usize;
        let size = header.filesz as usize;
        
        if offset + size > self.data.len() {
            return Err(ElfError::InvalidProgramHeader);
        }
        
        Ok(&self.data[offset..offset + size])
    }
    
    /// Get interpreter path (for dynamic executables)
    pub fn interpreter(&self) -> ElfResult<Option<&'a str>> {
        let headers = self.program_headers()?;
        
        for header in headers {
            if header.typ == ProgType::Interp as u32 {
                let offset = header.offset as usize;
                let size = header.filesz as usize;
                
                if offset + size > self.data.len() {
                    return Err(ElfError::InvalidProgramHeader);
                }
                
                let bytes = &self.data[offset..offset + size];
                // Remove trailing null byte
                let path = if let Some(null_pos) = bytes.iter().position(|&b| b == 0) {
                    &bytes[..null_pos]
                } else {
                    bytes
                };
                
                return Ok(Some(core::str::from_utf8(path).ok().unwrap_or("")));
            }
        }
        
        Ok(None)
    }
}

/// Load ELF into memory
/// 
/// Phase 4C: Complete implementation
/// 
/// # Steps:
/// 1. Parse and validate ELF
/// 2. Load all PT_LOAD segments into memory
/// 3. Setup proper page protections (R/W/X)
/// 4. Return entry point
/// 
/// # Arguments
/// * `data` - The ELF file bytes
/// * `mapper` - Memory mapper for the target address space
/// 
/// # Returns
/// * Entry point address on success
pub fn load_elf_into_memory(
    data: &[u8],
    mapper: &mut crate::memory::virtual_mem::mapper::MemoryMapper,
) -> ElfResult<u64> {
    use crate::memory::{VirtualAddress, PhysicalAddress};
    use crate::memory::virtual_mem::page_table::PageTableFlags;
    use crate::memory::page_allocator::allocate_page;
    
    let elf = ElfFile::parse(data)?;
    
    crate::logger::info(&alloc::format!(
        "[ELF] Loading ELF: entry=0x{:x}, type={}, {} segments",
        elf.entry_point(),
        elf.header().typ,
        elf.header().phnum
    ));
    
    // Charger tous les segments PT_LOAD
    let loadable = elf.loadable_segments()?;
    let mut segment_count = 0;
    
    for segment in loadable {
        if segment.memsz == 0 {
            continue; // Skip empty segments
        }
        
        segment_count += 1;
        
        // Calculer les adresses et tailles alignées sur pages
        let virt_start = segment.vaddr as usize;
        let virt_end = virt_start + segment.memsz as usize;
        let file_size = segment.filesz as usize;
        
        let page_size = crate::arch::PAGE_SIZE;
        let virt_start_aligned = virt_start & !(page_size - 1);
        let virt_end_aligned = (virt_end + page_size - 1) & !(page_size - 1);
        let num_pages = (virt_end_aligned - virt_start_aligned) / page_size;
        
        crate::logger::debug(&alloc::format!(
            "[ELF]   Segment: vaddr=0x{:x}, memsz=0x{:x}, filesz=0x{:x}, {} pages",
            virt_start, segment.memsz, file_size, num_pages
        ));
        
        // Déterminer les flags de page à partir des flags de segment
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER;
        if segment.flags & 0x1 != 0 { // PF_X
            flags |= PageTableFlags::EXECUTABLE;
        }
        if segment.flags & 0x2 != 0 { // PF_W
            flags |= PageTableFlags::WRITABLE;
        }
        // PF_R (0x4) est implicite avec PRESENT
        
        // Allouer et mapper les pages
        for page_idx in 0..num_pages {
            let page_virt = virt_start_aligned + page_idx * page_size;
            let page_phys = allocate_page()
                .map_err(|_| ElfError::InvalidProgramHeader)?; // TODO: Better error
            
            // Mapper la page
            mapper.map_page(
                VirtualAddress::new(page_virt),
                page_phys,
                flags
            ).map_err(|_| ElfError::InvalidProgramHeader)?;
            
            // Copier les données du segment dans cette page
            let page_offset_in_segment = if page_virt >= virt_start {
                page_virt - virt_start
            } else {
                0
            };
            
            let copy_start_in_page = if page_virt < virt_start {
                virt_start - page_virt
            } else {
                0
            };
            
            let bytes_to_copy = core::cmp::min(
                page_size - copy_start_in_page,
                file_size.saturating_sub(page_offset_in_segment)
            );
            
            if bytes_to_copy > 0 {
                let seg_data = elf.segment_data(segment)?;
                let src = &seg_data[page_offset_in_segment..page_offset_in_segment + bytes_to_copy];
                
                // TODO: Utiliser des mappings temporaires
                // Pour l'instant on assume identity mapping
                let dst_ptr = (page_phys.value() + copy_start_in_page) as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr(),
                        dst_ptr,
                        bytes_to_copy
                    );
                    
                    // Zéro le reste de la page (BSS)
                    if bytes_to_copy < page_size - copy_start_in_page {
                        core::ptr::write_bytes(
                            dst_ptr.add(bytes_to_copy),
                            0,
                            page_size - copy_start_in_page - bytes_to_copy
                        );
                    }
                }
            } else {
                // Page BSS (zéro)
                let dst_ptr = page_phys.value() as *mut u8;
                unsafe {
                    core::ptr::write_bytes(dst_ptr, 0, page_size);
                }
            }
        }
    }
    
    if segment_count == 0 {
        return Err(ElfError::NoLoadableSegments);
    }
    
    crate::logger::info(&alloc::format!(
        "[ELF] Successfully loaded {} segments, entry point: 0x{:x}",
        segment_count,
        elf.entry_point()
    ));
    
    Ok(elf.entry_point())
}

/// Simple wrapper for backward compatibility
pub fn load_elf(data: &[u8]) -> ElfResult<u64> {
    let elf = ElfFile::parse(data)?;
    
    crate::logger::info(&alloc::format!(
        "[ELF] Parsed ELF: entry=0x{:x}, {} program headers",
        elf.entry_point(),
        elf.header().phnum
    ));
    
    Ok(elf.entry_point())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_elf_magic() {
        assert_eq!(ELF_MAGIC, [0x7F, b'E', b'L', b'F']);
    }
}
