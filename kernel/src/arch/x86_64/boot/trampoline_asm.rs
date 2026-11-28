//! # arch/x86_64/boot/trampoline_asm.rs — SMP Trampoline (Real Mode → 64-bit)
//!
//! The trampoline is a 16-bit code block copied to physical address 0x6000
//! (INIT SIPI page = 6). APs receive SIPI with this page and begin
//! execution in real mode at 0x6000:0x0000 = physical 0x6000.
//!
//! ## Transition
//! 1. Real mode (16-bit): load temporary GDT + enable PE
//! 2. Protected mode (32-bit): jump to 32-bit
//! 3. Enable Long Mode (EFER.LMA) + enable paging with BSP PML4
//! 4. Jump to 64-bit
//! 5. Load dedicated per-AP stack
//! 6. Call `ap_entry(cpu_id, lapic_id, kernel_stack_top)`
//!
//! ## Memory Layout (relative to 0x6000)
//! ```
//! 0x0000 : 16-bit code
//! 0x0010 : u32 handshake (BSP waits here for `AP_ALIVE_MAGIC` signature)
//! 0x0020 : u64 pml4_phys (BSP PML4 address, filled by install_trampoline)
//! 0x0028 : u32 cpu_count (counter — dynamically filled)
//! 0x0030 : 32-bit code
//! 0x0080 : 64-bit code
//! ```

use super::super::smp::init::TRAMPOLINE_PHYS;

// ── Trampoline Binary Image ───────────────────────────────────────────────

// The trampoline is written entirely in `global_asm!`.
// It is linked into the `.trampoline` section and copied to 0x6000 at runtime.

core::arch::global_asm!(
    // ── Trampoline section (16-bit -> 32-bit -> 64-bit at physical 0x6000) ──
    ".section .trampoline, \"awx\"",
    ".balign 256",
    ".code16",
    ".global trampoline_start",
    "trampoline_start:",
    // 0x6000: Configure 16-bit segments and jump past parameter/GDT block to 0x6080
    "cli",
    "cld",
    "xor ax, ax",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "jmp 1f",
    // 0x6010..0x603F: Parameter & handshake block populated by BSP
    //   0x6010: u32 handshake (AP_ALIVE_MAGIC)
    //   0x6018: u64 stack_top
    //   0x6020: u64 pml4_phys
    //   0x6028: u32 cpu_id
    //   0x602C: u32 lapic_id
    //   0x6030: u64 ap_entry_addr
    ".balign 0x80",
    "1:",
    // 0x6080 (16-bit): Load temporary GDTR at 0x6060 and enable CR0.PE
    "lgdt [0x6060]",
    "mov eax, cr0",
    "or eax, 1",
    "mov cr0, eax",
    // Far jump to 32-bit code at 0x60A0 (selector 0x08 = 32-bit code)
    ".byte 0xEA, 0xA0, 0x60, 0x08, 0x00",
    // ── 0x60A0: 32-bit Protected Mode ───────────────────────────────────────
    ".code32",
    ".balign 0x20",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "mov fs, ax",
    "mov gs, ax",
    // Enable PAE (bit 5) + OSFXSR (bit 9) + OSXMMEXCPT (bit 10)
    "mov eax, cr4",
    "or eax, 0x620",
    "mov cr4, eax",
    // Load BSP PML4 physical address (< 4 GiB) from 0x6020
    "mov eax, [0x6020]",
    "mov cr3, eax",
    // Enable Long Mode (EFER.LME bit 8 + EFER.NXE bit 11)
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or eax, 0x900",
    "wrmsr",
    // Enable Paging (CR0.PG bit 31) + Protected Mode (CR0.PE bit 0)
    "mov eax, cr0",
    "or eax, 0x80000001",
    "mov cr0, eax",
    // Far jump to 64-bit code at 0x6100 (selector 0x18 = 64-bit code)
    ".byte 0xEA, 0x00, 0x61, 0x00, 0x00, 0x18, 0x00",
    // ── 0x6100: 64-bit Long Mode ────────────────────────────────────────────
    ".code64",
    ".balign 0x40",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "xor ax, ax",
    "mov fs, ax",
    "mov gs, ax",
    // Load dedicated AP stack and arguments for ap_entry(cpu_id, lapic_id, stack_top)
    "mov rsp, [0x6018]",
    "mov edi, [0x6028]",
    "mov esi, [0x602C]",
    "mov rdx, rsp",
    "and rsp, -16",
    "mov rax, [0x6030]",
    "call rax",
    "2: hlt",
    "jmp 2b",
    ".global trampoline_end",
    "trampoline_end:",
    ".size trampoline_start, trampoline_end - trampoline_start",
    ".previous",
);

// Linker exports
extern "C" {
    static trampoline_start: u8;
    static trampoline_end: u8;
}

// ── Trampoline Installation ────────────────────────────────────────────────

/// Copies the trampoline to 0x6000 and configures PML4 + temporary GDT/GDTR
///
/// Called by `early_init` before `smp_boot_aps()`.
pub fn install_trampoline() {
    // SAFETY: trampoline_start/end are defined by the linker in .trampoline
    let src: *const u8 = unsafe { &trampoline_start };
    let end: *const u8 = unsafe { &trampoline_end };
    let size = unsafe { end.offset_from(src) as usize };

    let dst = TRAMPOLINE_PHYS as *mut u8;

    // SAFETY: 0x6000 is in free low memory, identity-mapped
    unsafe {
        core::ptr::write_bytes(dst, 0, 0x200);
        core::ptr::copy_nonoverlapping(src, dst, size);
    }

    // Write current PML4 to trampoline offset 0x20
    let pml4_phys: u64;
    // SAFETY: CR3 read — physical address of current PML4
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) pml4_phys, options(nostack, nomem));
    }
    // SAFETY: write PML4 and temporary GDT/GDTR into 0x6020..0x6066
    unsafe {
        core::ptr::write_volatile((TRAMPOLINE_PHYS + 0x20) as *mut u64, pml4_phys & 0xFFFF_FFFF_F000);
        // Temporary GDT at 0x6040:
        //   0x00: null
        //   0x08: 32-bit code (0x00CF_9A00_0000_FFFF)
        //   0x10: 32/64-bit data (0x00CF_9200_0000_FFFF)
        //   0x18: 64-bit code (0x00AF_9A00_0000_FFFF)
        core::ptr::write_volatile((TRAMPOLINE_PHYS + 0x40) as *mut u64, 0);
        core::ptr::write_volatile((TRAMPOLINE_PHYS + 0x48) as *mut u64, 0x00CF_9A00_0000_FFFF);
        core::ptr::write_volatile((TRAMPOLINE_PHYS + 0x50) as *mut u64, 0x00CF_9200_0000_FFFF);
        core::ptr::write_volatile((TRAMPOLINE_PHYS + 0x58) as *mut u64, 0x00AF_9A00_0000_FFFF);
        // Temporary GDTR at 0x6060 (limit = 31, base = 0x6040 — 2-byte aligned u32 at +0x62)
        core::ptr::write_volatile((TRAMPOLINE_PHYS + 0x60) as *mut u16, 31);
        core::ptr::write_unaligned((TRAMPOLINE_PHYS + 0x62) as *mut u32, (TRAMPOLINE_PHYS + 0x40) as u32);
    }
}
