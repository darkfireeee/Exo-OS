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
    // ── Trampoline section (16-bit) ─────────────────────────────────────────
    ".section .trampoline, \"awx\"",
    ".code16",
    ".global trampoline_start",
    "trampoline_start:",
    // Configure 16-bit segments
    "cli",
    "xor ax, ax",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    // Load temporary 32-bit GDTR (offset 0x50 in trampoline)
    "lgdt [0x6050]",
    // Enable Protected Mode (CR0.PE)
    "mov eax, cr0",
    "or eax, 1",
    "mov cr0, eax",
    // Jump to 32-bit (selector 0x08 = KERNEL_CS)
    // LLVM assembler does not support `ljmp imm16, imm16` in AT&T syntax.
    // Manual encoding: opcode EA + offset16-LE + seg16-LE = EA 80 60 08 00
    ".byte 0xEA, 0x80, 0x60, 0x08, 0x00", // ljmp 0x08:0x6080
    // ── 32-bit section ────────────────────────────────────────────────────
    ".code32",
    ".balign 0x80", // Align to 0x80 = offset 0x80 from trampoline_start
    // Load 32-bit segments
    "mov ax, 0x10", // KERNEL_DS = 0x10
    "mov ds, ax",
    "mov ss, ax",
    "mov es, ax",
    "mov fs, ax",
    "mov gs, ax",
    // Enable PAE (CR4.PAE, bit 5)
    "mov eax, cr4",
    "or eax, 0x620", // basic PAE(5) + OSFXSR(9) + OSXMMEXCPT(10)
    "mov cr4, eax",
    // Load BSP PML4 (64-bit address read from offset 0x6020)
    "mov eax, [0x6020]", // low address of PML4 (64-bit, but below 4GiB at boot)
    "mov cr3, eax",
    // Enable Long Mode in EFER (MSR 0xC0000080)
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or eax, 0x900", // LME(8) + NXE(11)
    "wrmsr",
    // Enable Paging (CR0.PG) + Protection (already done but repeating is safe)
    "mov eax, cr0",
    "or eax, 0x80000001",
    "mov cr0, eax",
    // Jump to 64-bit (selector 0x08 = KERNEL_CS64)
    // LLVM assembler does not support `ljmp imm16, imm32` in AT&T syntax.
    // Manual encoding (32-bit mode): EA + offset32-LE + seg16-LE
    ".byte 0xEA, 0xC0, 0x60, 0x00, 0x00, 0x08, 0x00", // ljmp 0x08:0x60C0
    // ── 64-bit section ────────────────────────────────────────────────────
    ".code64",
    ".balign 0x40", // align to 0xC0
    // Load dedicated AP stack (address at offset 0x6038 — filled at runtime)
    "mov rsp, [rip + 0f]",
    "jmp 1f",
    "0: .quad 0", // placeholder — filled by install_trampoline per-AP
    "1:",
    // Read cpu_id from offset 0x6028
    "mov edi, [0x6028]", // cpu_id (argument 1)
    "mov esi, [0x602C]", // lapic_id (argument 2)
    "mov rdx, rsp",      // kernel_stack_top (argument 3)
    // Align stack to 16 bytes
    "and rsp, -16",
    // Call ap_entry (symbol defined in smp/init.rs)
    "call ap_entry",
    // Must never return — halt if it occurs
    "2: hlt",
    "jmp 2b",
    // ── Temporary GDT (shared boot) ─────────────────────────────────────
    ".balign 8",
    ".global trampoline_gdt",
    "trampoline_gdt:",
    ".quad 0",                  // null
    ".quad 0x00AF9A000000FFFF", // KERNEL_CS64 : Long mode, executable
    ".quad 0x00CF92000000FFFF", // KERNEL_DS32 : 32-bit data
    ".global trampoline_gdtr",
    "trampoline_gdtr:",
    ".word 23",         // limit = 3 * 8 - 1
    ".long 0x6050 + 8", // base = trampoline_gdt address
    ".global trampoline_end",
    "trampoline_end:",
    ".size trampoline_start, trampoline_end - trampoline_start",
    ".previous", // return to .text
);

// Linker exports
extern "C" {
    static trampoline_start: u8;
    static trampoline_end: u8;
}

// ── Trampoline Installation ────────────────────────────────────────────────

/// Copies the trampoline to 0x6000 and configures PML4 pointers
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
        core::ptr::copy_nonoverlapping(src, dst, size);
    }

    // Write current PML4 to trampoline offset 0x20
    let pml4_phys: u64;
    // SAFETY: CR3 read — physical address of current PML4
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) pml4_phys, options(nostack, nomem));
    }
    let pml4_slot = (TRAMPOLINE_PHYS + 0x20) as *mut u64;
    // SAFETY: offset in copied trampoline
    unsafe {
        core::ptr::write_volatile(pml4_slot, pml4_phys & 0xFFFF_FFFF_F000);
    }

    // Write GDTR pointing to trampoline_gdt (offset 0x50)
    // The temporary GDT is already in the copied trampoline
}
