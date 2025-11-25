//! Global Descriptor Table (GDT)
//! 
//! Manages memory segmentation for x86_64.

use core::mem::size_of;

/// GDT Entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_middle: u8,
    access: u8,
    granularity: u8,
    base_high: u8,
}

impl GdtEntry {
    const fn null() -> Self {
        GdtEntry {
            limit_low: 0,
            base_low: 0,
            base_middle: 0,
            access: 0,
            granularity: 0,
            base_high: 0,
        }
    }

    const fn new(base: u32, limit: u32, access: u8, flags: u8) -> Self {
        GdtEntry {
            limit_low: (limit & 0xFFFF) as u16,
            base_low: (base & 0xFFFF) as u16,
            base_middle: ((base >> 16) & 0xFF) as u8,
            access,
            granularity: (((limit >> 16) & 0x0F) as u8) | ((flags & 0xF0)),
            base_high: ((base >> 24) & 0xFF) as u8,
        }
    }

    const fn kernel_code() -> Self {
        // Access: Present, Ring 0, Code, Execute/Read
        // Flags: Long mode, Page granularity
        Self::new(0, 0xFFFFF, 0x9A, 0xA0)
    }

    const fn kernel_data() -> Self {
        // Access: Present, Ring 0, Data, Read/Write
        // Flags: Long mode, Page granularity
        Self::new(0, 0xFFFFF, 0x92, 0xC0)
    }

    const fn user_code() -> Self {
        // Access: Present, Ring 3, Code, Execute/Read
        Self::new(0, 0xFFFFF, 0xFA, 0xA0)
    }

    const fn user_data() -> Self {
        // Access: Present, Ring 3, Data, Read/Write
        Self::new(0, 0xFFFFF, 0xF2, 0xC0)
    }
}

/// GDT with 5 entries (null, kernel code, kernel data, user code, user data)
#[repr(C, packed)]
pub struct GDT {
    entries: [GdtEntry; 5],
}

impl GDT {
    const fn new() -> Self {
        GDT {
            entries: [
                GdtEntry::null(),
                GdtEntry::kernel_code(),
                GdtEntry::kernel_data(),
                GdtEntry::user_code(),
                GdtEntry::user_data(),
            ],
        }
    }
}

/// GDT Pointer (for LGDT instruction)
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

static mut GDT: GDT = GDT::new();

/// Initialize GDT
pub fn init() {
    unsafe {
        let gdt_ptr = GdtPointer {
            limit: (size_of::<GDT>() - 1) as u16,
            base: &GDT as *const _ as u64,
        };

        // Load GDT
        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_ptr,
            options(nostack)
        );

        // Reload segment registers
        core::arch::asm!(
            "push 0x08",      // Kernel code selector
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, 0x10",   // Kernel data selector
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",
            out("rax") _,
        );
    }
}

/// Selectors
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const USER_CODE_SELECTOR: u16 = 0x18;
pub const USER_DATA_SELECTOR: u16 = 0x20;
