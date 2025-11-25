// Multiboot2 information parser
// Parses the Multiboot2 information structure passed by the bootloader

use core::slice;

/// Multiboot2 information structure
#[repr(C)]
pub struct MultibootInfo {
    pub total_size: u32,
    _reserved: u32,
}

/// Multiboot2 tag types
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    End = 0,
    CommandLine = 1,
    BootloaderName = 2,
    Module = 3,
    BasicMemInfo = 4,
    BootDevice = 5,
    MemoryMap = 6,
    VbeInfo = 7,
    FramebufferInfo = 8,
    ElfSections = 9,
    ApmTable = 10,
    Efi32 = 11,
    Efi64 = 12,
    Smbios = 13,
    AcpiOld = 14,
    AcpiNew = 15,
    Network = 16,
    EfiMemoryMap = 17,
    EfiBs = 18,
    Efi32ImageHandle = 19,
    Efi64ImageHandle = 20,
    LoadBaseAddr = 21,
}

/// Generic tag header
#[repr(C)]
pub struct Tag {
    pub typ: u32,
    pub size: u32,
}

/// Command line tag
#[repr(C)]
pub struct CommandLineTag {
    pub typ: u32,
    pub size: u32,
    // string follows
}

/// Bootloader name tag
#[repr(C)]
pub struct BootloaderNameTag {
    pub typ: u32,
    pub size: u32,
    // string follows
}

/// Basic memory info tag
#[repr(C)]
pub struct BasicMemInfoTag {
    pub typ: u32,
    pub size: u32,
    pub mem_lower: u32,  // KB of lower memory
    pub mem_upper: u32,  // KB of upper memory
}

/// Memory map entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapEntry {
    pub base_addr: u64,
    pub length: u64,
    pub typ: u32,
    _reserved: u32,
}

/// Memory map tag
#[repr(C, packed)]
pub struct MemoryMapTag {
    pub typ: u32,
    pub size: u32,
    pub entry_size: u32,
    pub entry_version: u32,
    // entries follow
}

/// Parse the Multiboot2 information structure
pub unsafe fn parse(multiboot_addr: u64) -> Result<MultibootInfoParsed, &'static str> {
    if multiboot_addr == 0 {
        return Err("Null multiboot address");
    }
    
    // Check alignment
    if multiboot_addr % 8 != 0 {
        return Err("Multiboot info not 8-byte aligned");
    }
    
    let info = &*(multiboot_addr as *const MultibootInfo);
    
    if info.total_size < 8 {
        return Err("Invalid multiboot info size");
    }
    
    let mut parsed = MultibootInfoParsed::default();
    parsed.total_size = info.total_size;
    
    // Iterate through tags
    let mut offset = 8usize; // Skip header
    while offset < info.total_size as usize {
        let tag_addr = multiboot_addr + offset as u64;
        let tag = &*(tag_addr as *const Tag);
        
        if tag.typ == TagType::End as u32 {
            break;
        }
        
        // Parse specific tags
        match tag.typ {
            1 => { // CommandLine
                let cmd_tag = &*(tag_addr as *const CommandLineTag);
                let str_len = (cmd_tag.size - 8) as usize;
                if str_len > 0 {
                    let str_ptr = (tag_addr + 8) as *const u8;
                    let bytes = slice::from_raw_parts(str_ptr, str_len);
                    if let Ok(s) = core::str::from_utf8(bytes) {
                        parsed.command_line = Some(s.trim_end_matches('\0'));
                    }
                }
            }
            2 => { // BootloaderName
                let name_tag = &*(tag_addr as *const BootloaderNameTag);
                let str_len = (name_tag.size - 8) as usize;
                if str_len > 0 {
                    let str_ptr = (tag_addr + 8) as *const u8;
                    let bytes = slice::from_raw_parts(str_ptr, str_len);
                    if let Ok(s) = core::str::from_utf8(bytes) {
                        parsed.bootloader_name = Some(s.trim_end_matches('\0'));
                    }
                }
            }
            4 => { // BasicMemInfo
                let mem_tag = &*(tag_addr as *const BasicMemInfoTag);
                parsed.mem_lower_kb = Some(mem_tag.mem_lower);
                parsed.mem_upper_kb = Some(mem_tag.mem_upper);
            }
            6 => { // MemoryMap
                let mmap_tag = &*(tag_addr as *const MemoryMapTag);
                parsed.memory_map_addr = Some(tag_addr + 16);
                parsed.memory_map_entry_size = mmap_tag.entry_size;
                let num_entries = (mmap_tag.size - 16) / mmap_tag.entry_size;
                parsed.memory_map_entries = num_entries;
            }
            _ => {}
        }
        
        // Move to next tag (align to 8 bytes)
        offset += ((tag.size + 7) & !7) as usize;
    }
    
    Ok(parsed)
}

/// Parsed Multiboot2 information
#[derive(Default)]
pub struct MultibootInfoParsed {
    pub total_size: u32,
    pub command_line: Option<&'static str>,
    pub bootloader_name: Option<&'static str>,
    pub mem_lower_kb: Option<u32>,
    pub mem_upper_kb: Option<u32>,
    pub memory_map_addr: Option<u64>,
    pub memory_map_entry_size: u32,
    pub memory_map_entries: u32,
}

impl MultibootInfoParsed {
    /// Get total memory in KB
    pub fn total_memory_kb(&self) -> Option<u32> {
        if let (Some(lower), Some(upper)) = (self.mem_lower_kb, self.mem_upper_kb) {
            Some(lower + upper)
        } else {
            None
        }
    }
    
    /// Iterate over memory map entries
    pub unsafe fn memory_map_iter(&self) -> Option<MemoryMapIterator> {
        if let Some(addr) = self.memory_map_addr {
            Some(MemoryMapIterator {
                current: addr,
                remaining: self.memory_map_entries,
                entry_size: self.memory_map_entry_size,
            })
        } else {
            None
        }
    }
}

/// Iterator over memory map entries
pub struct MemoryMapIterator {
    current: u64,
    remaining: u32,
    entry_size: u32,
}

impl Iterator for MemoryMapIterator {
    type Item = MemoryMapEntry;
    
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        
        unsafe {
            let entry = *(self.current as *const MemoryMapEntry);
            self.current += self.entry_size as u64;
            self.remaining -= 1;
            Some(entry)
        }
    }
}

/// Memory region type
impl MemoryMapEntry {
    pub fn is_available(&self) -> bool {
        self.typ == 1
    }
    
    pub fn is_reserved(&self) -> bool {
        self.typ == 2
    }
    
    pub fn is_acpi_reclaimable(&self) -> bool {
        self.typ == 3
    }
    
    pub fn is_acpi_nvs(&self) -> bool {
        self.typ == 4
    }
    
    pub fn is_bad_memory(&self) -> bool {
        self.typ == 5
    }
    
    pub fn type_str(&self) -> &'static str {
        match self.typ {
            1 => "Available",
            2 => "Reserved",
            3 => "ACPI Reclaimable",
            4 => "ACPI NVS",
            5 => "Bad Memory",
            _ => "Unknown",
        }
    }
}
