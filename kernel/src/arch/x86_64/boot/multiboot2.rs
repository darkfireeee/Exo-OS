//! # arch/x86_64/boot/multiboot2.rs — Multiboot2 Parser
//!
//! Parses Multiboot2 tags to extract:
//! - Memory map (available memory, reserved, ACPI, etc.)
//! - RSDP address
//! - Bootloader name
//! - Command line
//!
//! ## Multiboot2 Format
//! Structure at the address passed by the bootloader:
//! - `total_size` (u32) + `reserved` (u32) = 8 header bytes
//! - Variable-length tags, aligned on 8 bytes
//! - Tag type 0 = end of list

// ── Multiboot2 Tag Types ──────────────────────────────────────────────────

const TAG_END: u32 = 0;
const TAG_CMDLINE: u32 = 1;
const TAG_BOOTLOADER: u32 = 2;
#[allow(dead_code)]
const TAG_MODULE: u32 = 3;
const TAG_BASIC_MEMINFO: u32 = 4;
const TAG_MMAP: u32 = 6;
#[allow(dead_code)]
const TAG_FRAMEBUFFER: u32 = 8;
#[allow(dead_code)]
const TAG_ELF_SECTIONS: u32 = 9;
#[allow(dead_code)]
const TAG_APM_TABLE: u32 = 10;
const TAG_RSDP_V1: u32 = 14; // ACPI 1.0 RSDP
const TAG_RSDP_V2: u32 = 15; // ACPI 2.0+ RSDP
#[allow(dead_code)]
const TAG_EFI_MMAP: u32 = 17;
#[allow(dead_code)]
const TAG_EFI64_IMAGE: u32 = 18;

// ── Memory map entry types ────────────────────────────────────────────────────

pub const MMAP_AVAILABLE: u32 = 1;
pub const MMAP_RESERVED: u32 = 2;
pub const MMAP_ACPI: u32 = 3;
pub const MMAP_HIBERNATE: u32 = 4;
pub const MMAP_DEFECTIVE: u32 = 5;

// ── Structures ────────────────────────────────────────────────────────────────

#[repr(C, packed)]
struct Mb2Header {
    total_size: u32,
    reserved: u32,
}

#[repr(C, packed)]
struct Mb2Tag {
    tag_type: u32,
    size: u32,
}

/// Multiboot2 memory map entry
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MmapEntry {
    pub base_addr: u64,
    pub length: u64,
    pub entry_type: u32,
    pub _reserved: u32,
}

/// Information extracted from the Multiboot2 structure
#[derive(Debug, Clone, Copy)]
pub struct Multiboot2Info {
    pub rsdp_phys: u64,
    pub total_memory_kb: u64,
    pub cmdline_ptr: u64, // pointer to null-terminated string
    pub bootloader_ptr: u64,
    pub mmap_ptr: u64, // pointer to the first MmapEntry
    pub mmap_count: u32,
    pub framebuffer_phys_addr: u64,
    pub framebuffer_width: u32,
    pub framebuffer_height: u32,
    pub framebuffer_stride: u32, // pixels per line
    pub framebuffer_bpp: u32,
    pub framebuffer_format: MultibootFramebufferFormat,
    pub framebuffer_size_bytes: u64,
    pub module_start: u64,
    pub module_end: u64,
    pub module_cmdline_ptr: u64,
}

/// Pixel format reported by the Multiboot2 framebuffer tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultibootFramebufferFormat {
    None,
    Rgbx,
    Bgrx,
    Unknown,
}

impl Multiboot2Info {
    const fn zeroed() -> Self {
        Self {
            rsdp_phys: 0,
            total_memory_kb: 0,
            cmdline_ptr: 0,
            bootloader_ptr: 0,
            mmap_ptr: 0,
            mmap_count: 0,
            framebuffer_phys_addr: 0,
            framebuffer_width: 0,
            framebuffer_height: 0,
            framebuffer_stride: 0,
            framebuffer_bpp: 0,
            framebuffer_format: MultibootFramebufferFormat::None,
            framebuffer_size_bytes: 0,
            module_start: 0,
            module_end: 0,
            module_cmdline_ptr: 0,
        }
    }
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Parses the Multiboot2 structure at physical address `mb2_phys`
///
/// # Safety
/// The address must be valid and in identity-mapped memory.
pub fn parse_multiboot2(mb2_phys: u64) -> Multiboot2Info {
    let mut info = Multiboot2Info::zeroed();
    if mb2_phys == 0 {
        return info;
    }

    // SAFETY: address passed by BSP bootloader, identity-mapped
    let header = unsafe { &*(mb2_phys as *const Mb2Header) };
    let total_size = header.total_size as usize;
    if total_size < 8 {
        return info;
    }

    let mut offset: usize = 8; // after Mb2Header

    while offset + 8 <= total_size {
        let tag_addr = mb2_phys as usize + offset;
        // SAFETY: offset within the Multiboot2 structure (total_size checked)
        let tag = unsafe { &*(tag_addr as *const Mb2Tag) };
        if tag.tag_type == TAG_END {
            break;
        }

        let tag_size = tag.size as usize;
        if tag_size < 8 {
            break;
        }

        let data_addr = tag_addr + 8; // after 8 bytes type+size

        match tag.tag_type {
            TAG_CMDLINE => {
                info.cmdline_ptr = data_addr as u64;
            }
            TAG_BOOTLOADER => {
                info.bootloader_ptr = data_addr as u64;
            }
            TAG_MODULE if tag_size >= 16 => {
                let mod_start =
                    unsafe { core::ptr::read_unaligned(data_addr as *const u32) } as u64;
                let mod_end =
                    unsafe { core::ptr::read_unaligned((data_addr + 4) as *const u32) } as u64;
                info.module_start = mod_start;
                info.module_end = mod_end;
                if tag_size > 16 {
                    info.module_cmdline_ptr = (data_addr + 8) as u64;
                }
            }
            TAG_BASIC_MEMINFO => {
                // u32 mem_lower (KiB below 1MiB) + u32 mem_upper (KiB above)
                // SAFETY: known fixed size
                let lower = unsafe { core::ptr::read_volatile(data_addr as *const u32) } as u64;
                let upper =
                    unsafe { core::ptr::read_volatile((data_addr + 4) as *const u32) } as u64;
                info.total_memory_kb = lower + upper;
            }
            TAG_MMAP => {
                // u32 entry_size + u32 entry_version + entries...
                // SAFETY: data_addr is within the validated MB2 tag region (tag_size > 8); volatile read of aligned u32.
                let entry_size =
                    unsafe { core::ptr::read_volatile(data_addr as *const u32) } as usize;
                if entry_size < core::mem::size_of::<MmapEntry>() {
                    break;
                }
                let entries_start = data_addr + 8;
                let entries_end = tag_addr + tag_size;
                let n_entries = (entries_end - entries_start) / entry_size;
                info.mmap_ptr = entries_start as u64;
                info.mmap_count = n_entries as u32;

                // Compute total available memory
                for i in 0..n_entries {
                    let entry_addr = entries_start + i * entry_size;
                    // SAFETY: index in the memory table
                    let entry = unsafe { &*(entry_addr as *const MmapEntry) };
                    if entry.entry_type == MMAP_AVAILABLE {
                        info.total_memory_kb += entry.length / 1024;
                    }
                }
            }
            TAG_RSDP_V1 | TAG_RSDP_V2 => {
                info.rsdp_phys = data_addr as u64;
            }
            TAG_FRAMEBUFFER if tag_size >= 30 => {
                // SAFETY: the tag was already validated against total_size.
                let fb_addr = unsafe { core::ptr::read_unaligned(data_addr as *const u64) };
                // SAFETY: fixed offsets in the MB2 framebuffer tag.
                let fb_pitch =
                    unsafe { core::ptr::read_unaligned((data_addr + 8) as *const u32) };
                let fb_width =
                    unsafe { core::ptr::read_unaligned((data_addr + 12) as *const u32) };
                let fb_height =
                    unsafe { core::ptr::read_unaligned((data_addr + 16) as *const u32) };
                let fb_bpp =
                    unsafe { core::ptr::read_unaligned((data_addr + 20) as *const u8) };
                let fb_type =
                    unsafe { core::ptr::read_unaligned((data_addr + 21) as *const u8) };
                let bytes_per_pixel = ((fb_bpp as u32).saturating_add(7) / 8).max(1);

                let fb_format = if fb_type == 1 && tag_size >= 36 {
                    // Direct RGB: 6 additional bytes.
                    let red_pos =
                        unsafe { core::ptr::read_unaligned((data_addr + 24) as *const u8) };
                    let red_size =
                        unsafe { core::ptr::read_unaligned((data_addr + 25) as *const u8) };
                    let green_pos =
                        unsafe { core::ptr::read_unaligned((data_addr + 26) as *const u8) };
                    let green_size =
                        unsafe { core::ptr::read_unaligned((data_addr + 27) as *const u8) };
                    let blue_pos =
                        unsafe { core::ptr::read_unaligned((data_addr + 28) as *const u8) };
                    let blue_size =
                        unsafe { core::ptr::read_unaligned((data_addr + 29) as *const u8) };

                    if fb_bpp >= 24
                        && red_size == 8
                        && green_size == 8
                        && blue_size == 8
                        && red_pos == 16
                        && green_pos == 8
                        && blue_pos == 0
                    {
                        MultibootFramebufferFormat::Rgbx
                    } else if fb_bpp >= 24
                        && red_size == 8
                        && green_size == 8
                        && blue_size == 8
                        && red_pos == 0
                        && green_pos == 8
                        && blue_pos == 16
                    {
                        MultibootFramebufferFormat::Bgrx
                    } else {
                        MultibootFramebufferFormat::Unknown
                    }
                } else {
                    MultibootFramebufferFormat::Unknown
                };

                info.framebuffer_phys_addr = fb_addr;
                info.framebuffer_width = fb_width;
                info.framebuffer_height = fb_height;
                info.framebuffer_stride = fb_pitch.checked_div(bytes_per_pixel).unwrap_or(0);
                info.framebuffer_bpp = fb_bpp as u32;
                info.framebuffer_format = fb_format;
                info.framebuffer_size_bytes =
                    (fb_pitch as u64).saturating_mul(fb_height as u64);
            }
            _ => {}
        }

        // Align next offset to 8 bytes
        offset += (tag_size + 7) & !7;
    }

    info
}

/// Returns a slice of memory map entries from a `Multiboot2Info`
pub fn mmap_entries(info: &Multiboot2Info) -> &'static [MmapEntry] {
    if info.mmap_ptr == 0 || info.mmap_count == 0 {
        return &[];
    }
    // SAFETY: addresses validated during parsing
    unsafe {
        core::slice::from_raw_parts(info.mmap_ptr as *const MmapEntry, info.mmap_count as usize)
    }
}

static MULTIBOOT2_MODULE: spin::Mutex<Option<(u64, u64)>> = spin::Mutex::new(None);

pub fn set_multiboot2_module(start: u64, end: u64) {
    if start != 0 && end > start {
        *MULTIBOOT2_MODULE.lock() = Some((start, end));
    }
}

pub fn get_multiboot2_module() -> Option<(u64, u64)> {
    *MULTIBOOT2_MODULE.lock()
}

