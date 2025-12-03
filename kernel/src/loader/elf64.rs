//! ELF64 Structure Definitions
//! 
//! Based on System V ABI for AMD64

use super::{ElfError, ElfResult};

/// ELF magic number
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF Class: 64-bit
pub const ELFCLASS64: u8 = 2;

/// ELF Data: Little Endian
pub const ELFDATA2LSB: u8 = 1;

/// ELF Machine: x86-64
pub const EM_X86_64: u16 = 62;

/// ELF Type values
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ElfType {
    None = 0,
    Rel = 1,
    Exec = 2,
    Dyn = 3,
    Core = 4,
}

/// Program header types
pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_SHLIB: u32 = 5;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474e550;
pub const PT_GNU_STACK: u32 = 0x6474e551;
pub const PT_GNU_RELRO: u32 = 0x6474e552;

/// Segment permission flags
pub const PF_X: u32 = 0x1;  // Execute
pub const PF_W: u32 = 0x2;  // Write
pub const PF_R: u32 = 0x4;  // Read

/// Section header types
pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_HASH: u32 = 5;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_NOTE: u32 = 7;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_DYNSYM: u32 = 11;

/// x86_64 relocation types
pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_GOT32: u32 = 3;
pub const R_X86_64_PLT32: u32 = 4;
pub const R_X86_64_COPY: u32 = 5;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_GOTPCREL: u32 = 9;
pub const R_X86_64_32: u32 = 10;
pub const R_X86_64_32S: u32 = 11;
pub const R_X86_64_16: u32 = 12;
pub const R_X86_64_PC16: u32 = 13;
pub const R_X86_64_8: u32 = 14;
pub const R_X86_64_PC8: u32 = 15;
pub const R_X86_64_TPOFF64: u32 = 18;
pub const R_X86_64_TPOFF32: u32 = 23;

/// ELF64 File Header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    /// Magic number and identification
    pub e_ident: [u8; 16],
    /// Object file type
    pub e_type: u16,
    /// Architecture
    pub e_machine: u16,
    /// Object file version
    pub e_version: u32,
    /// Entry point virtual address
    pub e_entry: u64,
    /// Program header table file offset
    pub e_phoff: u64,
    /// Section header table file offset
    pub e_shoff: u64,
    /// Processor-specific flags
    pub e_flags: u32,
    /// ELF header size in bytes
    pub e_ehsize: u16,
    /// Program header table entry size
    pub e_phentsize: u16,
    /// Program header table entry count
    pub e_phnum: u16,
    /// Section header table entry size
    pub e_shentsize: u16,
    /// Section header table entry count
    pub e_shnum: u16,
    /// Section name string table index
    pub e_shstrndx: u16,
}

impl Elf64Header {
    /// Parse an ELF64 header from bytes
    pub fn parse(data: &[u8]) -> ElfResult<Self> {
        if data.len() < 64 {
            return Err(ElfError::BufferTooSmall);
        }
        
        // Check magic
        if data[0..4] != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }
        
        // Check class (64-bit)
        if data[4] != ELFCLASS64 {
            return Err(ElfError::Not64Bit);
        }
        
        // Check endianness (little)
        if data[5] != ELFDATA2LSB {
            return Err(ElfError::NotLittleEndian);
        }
        
        // Parse header fields
        let header = Self {
            e_ident: {
                let mut ident = [0u8; 16];
                ident.copy_from_slice(&data[0..16]);
                ident
            },
            e_type: u16::from_le_bytes([data[16], data[17]]),
            e_machine: u16::from_le_bytes([data[18], data[19]]),
            e_version: u32::from_le_bytes([data[20], data[21], data[22], data[23]]),
            e_entry: u64::from_le_bytes([
                data[24], data[25], data[26], data[27],
                data[28], data[29], data[30], data[31],
            ]),
            e_phoff: u64::from_le_bytes([
                data[32], data[33], data[34], data[35],
                data[36], data[37], data[38], data[39],
            ]),
            e_shoff: u64::from_le_bytes([
                data[40], data[41], data[42], data[43],
                data[44], data[45], data[46], data[47],
            ]),
            e_flags: u32::from_le_bytes([data[48], data[49], data[50], data[51]]),
            e_ehsize: u16::from_le_bytes([data[52], data[53]]),
            e_phentsize: u16::from_le_bytes([data[54], data[55]]),
            e_phnum: u16::from_le_bytes([data[56], data[57]]),
            e_shentsize: u16::from_le_bytes([data[58], data[59]]),
            e_shnum: u16::from_le_bytes([data[60], data[61]]),
            e_shstrndx: u16::from_le_bytes([data[62], data[63]]),
        };
        
        // Validate architecture
        if header.e_machine != EM_X86_64 {
            return Err(ElfError::WrongArchitecture);
        }
        
        Ok(header)
    }
    
    /// Check if this is a Position Independent Executable
    pub fn is_pie(&self) -> bool {
        self.e_type == ElfType::Dyn as u16
    }
    
    /// Check if this is a static executable
    pub fn is_static(&self) -> bool {
        self.e_type == ElfType::Exec as u16
    }
}

/// ELF64 Program Header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    /// Segment type
    pub p_type: u32,
    /// Segment flags
    pub p_flags: u32,
    /// Segment file offset
    pub p_offset: u64,
    /// Segment virtual address
    pub p_vaddr: u64,
    /// Segment physical address
    pub p_paddr: u64,
    /// Segment size in file
    pub p_filesz: u64,
    /// Segment size in memory
    pub p_memsz: u64,
    /// Segment alignment
    pub p_align: u64,
}

impl Elf64ProgramHeader {
    /// Parse a program header from bytes
    pub fn parse(data: &[u8]) -> ElfResult<Self> {
        if data.len() < 56 {
            return Err(ElfError::BufferTooSmall);
        }
        
        Ok(Self {
            p_type: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            p_flags: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            p_offset: u64::from_le_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]),
            p_vaddr: u64::from_le_bytes([
                data[16], data[17], data[18], data[19],
                data[20], data[21], data[22], data[23],
            ]),
            p_paddr: u64::from_le_bytes([
                data[24], data[25], data[26], data[27],
                data[28], data[29], data[30], data[31],
            ]),
            p_filesz: u64::from_le_bytes([
                data[32], data[33], data[34], data[35],
                data[36], data[37], data[38], data[39],
            ]),
            p_memsz: u64::from_le_bytes([
                data[40], data[41], data[42], data[43],
                data[44], data[45], data[46], data[47],
            ]),
            p_align: u64::from_le_bytes([
                data[48], data[49], data[50], data[51],
                data[52], data[53], data[54], data[55],
            ]),
        })
    }
    
    /// Check if this segment is loadable
    pub fn is_loadable(&self) -> bool {
        self.p_type == PT_LOAD
    }
    
    /// Check if this segment is executable
    pub fn is_executable(&self) -> bool {
        self.p_flags & PF_X != 0
    }
    
    /// Check if this segment is writable
    pub fn is_writable(&self) -> bool {
        self.p_flags & PF_W != 0
    }
    
    /// Check if this segment is readable
    pub fn is_readable(&self) -> bool {
        self.p_flags & PF_R != 0
    }
}

/// ELF64 Section Header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64SectionHeader {
    /// Section name (string table index)
    pub sh_name: u32,
    /// Section type
    pub sh_type: u32,
    /// Section flags
    pub sh_flags: u64,
    /// Section virtual address at execution
    pub sh_addr: u64,
    /// Section file offset
    pub sh_offset: u64,
    /// Section size in bytes
    pub sh_size: u64,
    /// Link to another section
    pub sh_link: u32,
    /// Additional section information
    pub sh_info: u32,
    /// Section alignment
    pub sh_addralign: u64,
    /// Entry size if section holds table
    pub sh_entsize: u64,
}

impl Elf64SectionHeader {
    /// Parse a section header from bytes
    pub fn parse(data: &[u8]) -> ElfResult<Self> {
        if data.len() < 64 {
            return Err(ElfError::BufferTooSmall);
        }
        
        Ok(Self {
            sh_name: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            sh_type: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            sh_flags: u64::from_le_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]),
            sh_addr: u64::from_le_bytes([
                data[16], data[17], data[18], data[19],
                data[20], data[21], data[22], data[23],
            ]),
            sh_offset: u64::from_le_bytes([
                data[24], data[25], data[26], data[27],
                data[28], data[29], data[30], data[31],
            ]),
            sh_size: u64::from_le_bytes([
                data[32], data[33], data[34], data[35],
                data[36], data[37], data[38], data[39],
            ]),
            sh_link: u32::from_le_bytes([data[40], data[41], data[42], data[43]]),
            sh_info: u32::from_le_bytes([data[44], data[45], data[46], data[47]]),
            sh_addralign: u64::from_le_bytes([
                data[48], data[49], data[50], data[51],
                data[52], data[53], data[54], data[55],
            ]),
            sh_entsize: u64::from_le_bytes([
                data[56], data[57], data[58], data[59],
                data[60], data[61], data[62], data[63],
            ]),
        })
    }
}

/// ELF64 Symbol entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Symbol {
    /// Symbol name (string table index)
    pub st_name: u32,
    /// Symbol type and binding
    pub st_info: u8,
    /// Symbol visibility
    pub st_other: u8,
    /// Section index
    pub st_shndx: u16,
    /// Symbol value
    pub st_value: u64,
    /// Symbol size
    pub st_size: u64,
}

impl Elf64Symbol {
    /// Parse a symbol from bytes
    pub fn parse(data: &[u8]) -> ElfResult<Self> {
        if data.len() < 24 {
            return Err(ElfError::BufferTooSmall);
        }
        
        Ok(Self {
            st_name: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            st_info: data[4],
            st_other: data[5],
            st_shndx: u16::from_le_bytes([data[6], data[7]]),
            st_value: u64::from_le_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]),
            st_size: u64::from_le_bytes([
                data[16], data[17], data[18], data[19],
                data[20], data[21], data[22], data[23],
            ]),
        })
    }
    
    /// Get symbol binding
    pub fn binding(&self) -> u8 {
        self.st_info >> 4
    }
    
    /// Get symbol type
    pub fn symbol_type(&self) -> u8 {
        self.st_info & 0xF
    }
}

/// ELF64 Relocation entry (with addend)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Rela {
    /// Location to apply relocation
    pub r_offset: u64,
    /// Relocation type and symbol index
    pub r_info: u64,
    /// Addend
    pub r_addend: i64,
}

impl Elf64Rela {
    /// Parse a relocation from bytes
    pub fn parse(data: &[u8]) -> ElfResult<Self> {
        if data.len() < 24 {
            return Err(ElfError::BufferTooSmall);
        }
        
        Ok(Self {
            r_offset: u64::from_le_bytes([
                data[0], data[1], data[2], data[3],
                data[4], data[5], data[6], data[7],
            ]),
            r_info: u64::from_le_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]),
            r_addend: i64::from_le_bytes([
                data[16], data[17], data[18], data[19],
                data[20], data[21], data[22], data[23],
            ]),
        })
    }
    
    /// Get symbol index
    pub fn symbol(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
    
    /// Get relocation type
    pub fn reloc_type(&self) -> u32 {
        (self.r_info & 0xFFFFFFFF) as u32
    }
}

/// Dynamic entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Dyn {
    /// Dynamic entry type
    pub d_tag: i64,
    /// Value or address
    pub d_val: u64,
}

impl Elf64Dyn {
    /// Parse a dynamic entry from bytes
    pub fn parse(data: &[u8]) -> ElfResult<Self> {
        if data.len() < 16 {
            return Err(ElfError::BufferTooSmall);
        }
        
        Ok(Self {
            d_tag: i64::from_le_bytes([
                data[0], data[1], data[2], data[3],
                data[4], data[5], data[6], data[7],
            ]),
            d_val: u64::from_le_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]),
        })
    }
}

// Dynamic entry tags
pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_PLTGOT: i64 = 3;
pub const DT_HASH: i64 = 4;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SYMENT: i64 = 11;
pub const DT_INIT: i64 = 12;
pub const DT_FINI: i64 = 13;
pub const DT_SONAME: i64 = 14;
pub const DT_RPATH: i64 = 15;
pub const DT_SYMBOLIC: i64 = 16;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_PLTREL: i64 = 20;
pub const DT_DEBUG: i64 = 21;
pub const DT_TEXTREL: i64 = 22;
pub const DT_JMPREL: i64 = 23;
pub const DT_BIND_NOW: i64 = 24;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
