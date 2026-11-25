//! # arch/x86_64/boot/multiboot2_trampoline.rs — 32→64-bit Boot Trampoline & BSP Stack
//!
//! Houses the Multiboot2 header, early identity page tables (`_boot_pml4`..`_boot_pd_high`),
//! minimal 64-bit GDT, `_start` (GRUB 32-bit) / `_start_uefi` (`exo-boot` 64-bit) /
//! `_start64` entry trampoline, and the 512 KiB `.boot_stack`.

// ── Multiboot2 Header ─────────────────────────────────────────────────────────
// PATCH-P2-BOOT: Multiboot2 Header gated behind --features multiboot2_compat.
// DEPRECATED (Strata vision: UEFI-only via exo-boot SPEC-BOOTLOADER-GPT-STRATA.md).
// ACTIVE by default (default feature) to preserve current QEMU/GRUB workflow.
// GRUB searches for magic 0xE85250D6 within the first 32 KiB of the image.
// Must be in the `.multiboot2` section, which is FIRST in linker.ld.
#[cfg(feature = "multiboot2_compat")]
core::arch::global_asm!(
    ".section .multiboot2, \"a\"",
    ".align 8",
    ".long 0xE85250D6", // Magic Multiboot2
    ".long 0",          // Architecture i386 (GRUB enters in 32-bit prot. mode)
    ".long 48",         // Header length: 16 + fb tag(20) + pad(4) + end tag(8)
    ".long 0x17ADAEFA", // Checksum : -(0xE85250D6 + 0 + 48) mod 2^32
    ".short 5",
    ".short 0",
    ".long 20", // Framebuffer request tag
    ".long 1920",
    ".long 1080",
    ".long 32",
    ".long 0", // padding 8-byte alignment before end tag
    ".short 0",
    ".short 0",
    ".long 8", // End tag
);

// ── Boot Page Tables (BSS, zero-initialized, 4 KiB-aligned) ───────────────────
core::arch::global_asm!(
    ".section .bss",
    ".balign 4096",
    ".global _boot_pml4",
    "_boot_pml4:",
    ".space 4096",
    ".global _boot_pdpt",
    "_boot_pdpt:",
    ".space 4096",
    ".global _boot_pd",
    "_boot_pd:",
    ".space 4096",
    ".global _boot_pd_high",
    "_boot_pd_high:",
    ".space 4096",
    // Save Multiboot2 args (EAX/EBX) during 32→64 transition
    ".global _mb2_saved_magic",
    "_mb2_saved_magic:",
    ".long 0",
    ".global _mb2_saved_info",
    "_mb2_saved_info:",
    ".quad 0",
);

// ── Minimal 64-bit GDT for boot trampoline ───────────────────────────────────
core::arch::global_asm!(
    ".section .rodata",
    ".balign 16",
    ".global _boot_gdt",
    "_boot_gdt:",
    ".quad 0x0000000000000000", // 0x00 — null descriptor
    ".quad 0x00af9a000000ffff", // 0x08 — 64-bit code, DPL=0, L=1
    ".quad 0x00cf92000000ffff", // 0x10 — data,       DPL=0, D=1
    "_boot_gdt_end_marker:",
    ".global _boot_gdtr",
    "_boot_gdtr:",
    ".short 23",        // limit = 3*8 - 1 = 23
    ".long  _boot_gdt", // base (32-bit physical — kernel < 4 GiB)
);

// ── Boot trampoline: 32-bit → 64-bit (long mode) ──────────────────────────────
core::arch::global_asm!(
    ".section .text.boot",
    ".global _start",
    ".type _start, @function",
    ".code32",
    "_start:",
    "cli",
    "cld",
    "mov dword ptr [_mb2_saved_magic], eax",
    "mov dword ptr [_mb2_saved_info],  ebx",
    "mov dword ptr [_mb2_saved_info + 4], 0",
    "lea esp, [_exo_boot_stack_top]",
    "and esp, -16",
    "xor ecx, ecx",
    "_pd_loop:",
    "  cmp ecx, 512",
    "  jge _pd_done",
    "  mov eax, ecx",
    "  shl eax, 21",
    "  or  eax, 0x83",
    "  lea edi, [_boot_pd]",
    "  mov ebx, ecx",
    "  shl ebx, 3",
    "  add edi, ebx",
    "  mov dword ptr [edi],     eax",
    "  mov dword ptr [edi + 4], 0",
    "  inc ecx",
    "  jmp _pd_loop",
    "_pd_done:",
    "lea eax, [_boot_pd]",
    "or  eax, 0x03",
    "mov dword ptr [_boot_pdpt],     eax",
    "mov dword ptr [_boot_pdpt + 4], 0",
    // Write PD_high[502] = 0xFEC0009B (IOAPIC 2 MiB page, P+R/W+PS+PWT+PCD UC)
    "lea edi, [_boot_pd_high]",
    "add edi, 4016",
    "mov dword ptr [edi],     0xFEC0009B",
    "mov dword ptr [edi + 4], 0",
    // Write PD_high[503] = 0xFEE0009B (LAPIC 2 MiB page, P+R/W+PS+PWT+PCD UC)
    "lea edi, [_boot_pd_high]",
    "add edi, 4024",
    "mov dword ptr [edi],     0xFEE0009B",
    "mov dword ptr [edi + 4], 0",
    // PDPT[3] -> _boot_pd_high (MMIO APIC region); offset = 3*8 = 24
    "lea eax, [_boot_pd_high]",
    "or  eax, 0x03",
    "mov dword ptr [_boot_pdpt + 24],     eax",
    "mov dword ptr [_boot_pdpt + 28], 0",
    // PML4[256] -> _boot_pdpt (physmap 0xFFFF_8000_0000_0000)
    "lea eax, [_boot_pdpt]",
    "or  eax, 0x03",
    "mov dword ptr [_boot_pml4 + 2048],     eax",
    "mov dword ptr [_boot_pml4 + 2052], 0",
    // PML4[0] -> PDPT
    "lea eax, [_boot_pdpt]",
    "or  eax, 0x03",
    "mov dword ptr [_boot_pml4],     eax",
    "mov dword ptr [_boot_pml4 + 4], 0",
    "lgdt [_boot_gdtr]",
    "lea eax, [_boot_pml4]",
    "mov cr3, eax",
    "mov eax, cr4",
    "or  eax, 0x00000020",
    "mov cr4, eax",
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or  eax, 0x00000100",
    "wrmsr",
    "mov eax, cr0",
    "or  eax, 0x80000000",
    "mov cr0, eax",
    "lea eax, [_start64]",
    "push dword ptr 0x08",
    "push eax",
    "retf",
    "_boot_halt32:",
    "cli",
    "hlt",
    "jmp _boot_halt32",
    // _start_uefi: 64-bit entry for exo-boot.
    ".code64",
    ".global _start_uefi",
    ".type _start_uefi, @function",
    "_start_uefi:",
    "cli",
    "cld",
    "mov dword ptr [rip + _mb2_saved_magic], eax",
    "mov qword ptr [rip + _mb2_saved_info], rbx",
    "jmp _start64",
    // _start64: first 64-bit code after long mode transition
    ".global _start64",
    ".type _start64, @function",
    "_start64:",
    "mov al, 0x58", // 'X'
    "out 0xe9, al",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "xor ax, ax",
    "mov fs, ax",
    "mov gs, ax",
    "lea rsp, [rip + _exo_boot_stack_top]",
    "and rsp, -16",
    "mov edi, dword ptr [rip + _mb2_saved_magic]",
    "mov rsi, qword ptr [rip + _mb2_saved_info]",
    "xor edx, edx",
    "call kernel_main",
    "cli",
    "_start64_halt:",
    "hlt",
    "jmp _start64_halt",
    ".size _start, . - _start",
);

// ── BSP Boot Stack ──────────────────────────────────────────────────────────
core::arch::global_asm!(
    ".section .boot_stack, \"aw\", @nobits",
    ".balign 4096",
    ".global _exo_boot_stack_bottom",
    "_exo_boot_stack_bottom:",
    ".space 524288",
    ".global _exo_boot_stack_top",
    "_exo_boot_stack_top:",
    ".section .text",
);

extern "C" {
    fn _start();
    fn _start_uefi();
}

/// Ensures the linker pulls this compilation unit (and its `.multiboot2`,
/// `.text.boot`, and `.boot_stack` sections) from `libexo_os_kernel.rlib`.
#[inline(never)]
pub fn link_boot_trampoline_anchor() {
    let _ = core::hint::black_box(_start as *const ());
    let _ = core::hint::black_box(_start_uefi as *const ());
}
