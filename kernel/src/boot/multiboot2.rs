//! Multiboot2 Boot Protocol Parser
//! 
//! Parses Multiboot2 boot information structure passed by the bootloader.
//! Provides access to memory map, modules, framebuffer info, etc.

use core::slice;

/// Multiboot2 magic number (must be in EAX after boot)
pub const MULTIBOOT2_MAGIC: u32 = 0x36d76289;

/// Boot information structure
#[repr(C)]
pub struct BootInfo {
    total_size: u32,
    reserved: u32,
    // Followed by tags
}

/// Tag types
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    End = 0,
    CommandLine = 1,
    BootLoaderName = 2,
    Module = 3,
    BasicMemInfo = 4,
    BootDevice = 5,
    MemoryMap = 6,
    VBEInfo = 7,
    Framebuffer = 8,
    ElfSections = 9,
    APMTable = 10,
    EFI32 = 11,
    EFI64 = 12,
    SMBIOS = 13,
    ACPIOld = 14,
    ACPINew = 15,
    Network = 16,
    EFIMemoryMap = 17,
    EFIBS = 18,
    EFI32ImageHandle = 19,
    EFI64ImageHandle = 20,
    LoadBaseAddr = 21,
}

/// Generic tag header
#[repr(C)]
pub struct Tag {
    typ: u32,
    size: u32,
    // Followed by tag-specific data
}

/// Memory map entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapEntry {
    pub base_addr: u64,
    pub length: u64,
    pub typ: u32,
    pub reserved: u32,
}

impl MemoryMapEntry {
    pub fn is_available(&self) -> bool {
        self.typ == 1 // Type 1 = Available RAM
    }
}

/// Framebuffer info
#[repr(C, packed)]
pub struct FramebufferInfo {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    pub typ: u8,
    pub reserved: u16,
}

/// Module info
#[repr(C)]
pub struct ModuleInfo {
    mod_start: u32,
    mod_end: u32,
    // Followed by null-terminated string
}

/// Boot information parser
pub struct Multiboot2Info {
    addr: usize,
    total_size: u32,
}

impl Multiboot2Info {
    /// Create from address (passed by bootloader)
    /// 
    /// # Safety
    /// The address must point to valid Multiboot2 structure
    pub unsafe fn from_ptr(addr: usize) -> Option<Self> {
        if addr == 0 {
            return None;
        }

        let boot_info = &*(addr as *const BootInfo);
        let total_size = boot_info.total_size;

        if total_size < 8 {
            return None; // Invalid size
        }

        Some(Multiboot2Info {
            addr,
            total_size,
        })
    }

    /// Validate magic number
    pub fn validate_magic(magic: u32) -> bool {
        magic == MULTIBOOT2_MAGIC
    }

    /// Iterate over all tags
    pub fn tags(&self) -> TagIterator {
        TagIterator {
            current: self.addr + 8, // Skip header
            end: self.addr + self.total_size as usize,
        }
    }

    /// Find tag by type
    pub fn find_tag(&self, tag_type: TagType) -> Option<&Tag> {
        self.tags().find(|tag| tag.typ == tag_type as u32)
    }

    /// Get memory map
    pub fn memory_map(&self) -> Option<MemoryMapIterator> {
        let tag = self.find_tag(TagType::MemoryMap)?;
        
        unsafe {
            let entry_size = *((tag as *const Tag as usize + 8) as *const u32);
            let entry_version = *((tag as *const Tag as usize + 12) as *const u32);
            
            if entry_version != 0 {
                return None; // Unsupported version
            }

            let entries_start = (tag as *const Tag as usize) + 16;
            let entries_end = (tag as *const Tag as usize) + tag.size as usize;

            Some(MemoryMapIterator {
                current: entries_start,
                end: entries_end,
                entry_size,
            })
        }
    }

    /// Get framebuffer info
    pub fn framebuffer(&self) -> Option<&FramebufferInfo> {
        let tag = self.find_tag(TagType::Framebuffer)?;
        
        unsafe {
            let fb_ptr = ((tag as *const Tag as usize) + 8) as *const FramebufferInfo;
            Some(&*fb_ptr)
        }
    }

    /// Get total memory size (in bytes)
    pub fn total_memory(&self) -> u64 {
        self.memory_map()
            .map(|iter| {
                iter.filter(|e| e.is_available())
                    .map(|e| e.length)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Get bootloader name
    pub fn bootloader_name(&self) -> Option<&str> {
        let tag = self.find_tag(TagType::BootLoaderName)?;
        
        unsafe {
            let name_ptr = ((tag as *const Tag as usize) + 8) as *const u8;
            let name_len = tag.size as usize - 8 - 1; // -1 for null terminator
            
            let name_bytes = slice::from_raw_parts(name_ptr, name_len);
            core::str::from_utf8(name_bytes).ok()
        }
    }

    /// Get command line
    pub fn command_line(&self) -> Option<&str> {
        let tag = self.find_tag(TagType::CommandLine)?;
        
        unsafe {
            let cmd_ptr = ((tag as *const Tag as usize) + 8) as *const u8;
            let cmd_len = tag.size as usize - 8 - 1; // -1 for null terminator
            
            let cmd_bytes = slice::from_raw_parts(cmd_ptr, cmd_len);
            core::str::from_utf8(cmd_bytes).ok()
        }
    }
}

/// Iterator over tags
pub struct TagIterator {
    current: usize,
    end: usize,
}

impl Iterator for TagIterator {
    type Item = &'static Tag;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.end {
            return None;
        }

        unsafe {
            let tag = &*(self.current as *const Tag);
            
            if tag.typ == 0 {
                // End tag
                return None;
            }

            // Align to 8 bytes
            let size_aligned = (tag.size + 7) & !7;
            self.current += size_aligned as usize;

            Some(tag)
        }
    }
}

/// Iterator over memory map entries
pub struct MemoryMapIterator {
    current: usize,
    end: usize,
    entry_size: u32,
}

impl Iterator for MemoryMapIterator {
    type Item = &'static MemoryMapEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.end {
            return None;
        }

        unsafe {
            let entry = &*(self.current as *const MemoryMapEntry);
            self.current += self.entry_size as usize;
            Some(entry)
        }
    }
}

/// Print memory map (debug)
pub fn print_memory_map(boot_info: &Multiboot2Info) {
    if let Some(mmap) = boot_info.memory_map() {
        log::info!("Memory Map:");
        for (i, entry) in mmap.enumerate() {
            // Copy packed fields to avoid alignment issues
            let base = entry.base_addr;
            let len = entry.length;
            log::info!(
                "  [{:2}] 0x{:016x} - 0x{:016x} ({}MB) Type: {}",
                i,
                base,
                base + len,
                len / 1024 / 1024,
                if entry.is_available() { "Available" } else { "Reserved" }
            );
        }
    }

    let total = boot_info.total_memory();
    log::info!("Total Memory: {}MB", total / 1024 / 1024);
    
    if let Some(name) = boot_info.bootloader_name() {
        log::info!("Bootloader: {}", name);
    }

    if let Some(cmd) = boot_info.command_line() {
        log::info!("Command line: {}", cmd);
    }
}
