//! # arch/x86_64/boot/trampoline_asm.rs — Trampoline SMP (Real Mode → 64 bits)
//!
//! Le trampoline est un bloc de code 16 bits copié à l'adresse physique 0x6000
//! (page d'INIT SIPI = 6). Les APs reçoivent le SIPI avec cette page et commencent
//! l'exécution en mode réel à 0x6000:0x0000 = 0x6000 physique.
//!
//! ## Transition
//! 1. Mode réel (16 bits) : charger GDT temporaire + activer PE
//! 2. Mode protégé (32 bits) : sauter en 32 bits
//! 3. Activer Long Mode (EFER.LMA) + activer paging avec PML4 du BSP
//! 4. Sauter en 64 bits
//! 5. Charger la pile dédiée per-AP
//! 6. Appeler `ap_entry(cpu_id, lapic_id, kernel_stack_top)`
//!
//! ## Layout en mémoire (relatif à 0x6000)
//! ```
//! 0x0000 : code 16 bits
//! 0x0010 : u32 handshake (BSP attend ici la signature `AP_ALIVE_MAGIC`)
//! 0x0020 : u64 pml4_phys (adresse du PML4 du BSP, remplie par install_trampoline)
//! 0x0028 : u32 cpu_count (compteur — rempli dynamiquement)
//! 0x0030 : code 32 bits
//! 0x0080 : code 64 bits
//! ```

#![allow(dead_code)]

use super::super::smp::init::TRAMPOLINE_PHYS;

// ── Image binaire du trampoline ───────────────────────────────────────────────

// Le trampoline est écrit entièrement en `global_asm!`.
// Il sera linké dans la section `.trampoline` et copié à 0x6000 au runtime.

core::arch::global_asm!(
    // ── Section trampoline (16 bits) ─────────────────────────────────────────
    ".section .trampoline, \"awx\"",
    ".code16",
    ".global trampoline_start",
    "trampoline_start:",

    // Configurer les segments 16 bits
    "cli",
    "xor ax, ax",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",

    // Charger la GDTR temporaire 32 bits (offset 0x50 dans le trampoline)
    "lgdt [0x6050]",

    // Activer Protected Mode (CR0.PE)
    "mov eax, cr0",
    "or eax, 1",
    "mov cr0, eax",

    // Sauter en 32 bits (sélecteur 0x08 = KERNEL_CS)
    // LLVM assembler ne supporte pas `ljmp imm16, imm16` en AT&T syntax.
    // Encodage manuel : opcode EA + offset16-LE + seg16-LE = EA 80 60 08 00
    ".byte 0xEA, 0x80, 0x60, 0x08, 0x00",  // ljmp 0x08:0x6080

    // ── Section 32 bits ────────────────────────────────────────────────────
    ".code32",
    ".balign 0x80",         // Aligner sur 0x80 = offset 0x80 depuis trampoline_start

    // Charger segments 32 bits
    "mov ax, 0x10",         // KERNEL_DS = 0x10
    "mov ds, ax",
    "mov ss, ax",
    "mov es, ax",
    "mov fs, ax",
    "mov gs, ax",

    // Activer PAE (CR4.PAE, bit 5)
    "mov eax, cr4",
    "or eax, 0x620",        // PAE(5) + OSFXSR(9) + OSXMMEXCPT(10) basiques
    "mov cr4, eax",

    // Charger le PML4 du BSP (adresse 64 bits lue depuis l'offset 0x6020)
    "mov eax, [0x6020]",    // adresse basse du PML4 (64 bits, mais en dessous de 4GiB au boot)
    "mov cr3, eax",

    // Activer Long Mode dans EFER (MSR 0xC0000080)
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or eax, 0x900",        // LME(8) + NXE(11)
    "wrmsr",

    // Activer Paging (CR0.PG) + Protection (déjà fait mais répéter est safe)
    "mov eax, cr0",
    "or eax, 0x80000001",
    "mov cr0, eax",

    // Sauter en 64 bits (sélecteur 0x08 = KERNEL_CS64)
    // LLVM assembler ne supporte pas `ljmp imm16, imm32` en AT&T syntax.
    // Encodage manuel (mode 32 bits) : EA + offset32-LE + seg16-LE
    ".byte 0xEA, 0xC0, 0x60, 0x00, 0x00, 0x08, 0x00",  // ljmp 0x08:0x60C0

    // ── Section 64 bits ────────────────────────────────────────────────────
    ".code64",
    ".balign 0x40",         // aligner sur 0xC0

    // Charger la pile dédiée AP (adresse dans l'offset 0x6038 — rempli au runtime)
    "mov rsp, [rip + 0f]",
    "jmp 1f",
    "0: .quad 0",           // placeholder — rempli par install_trampoline per-AP
    "1:",

    // Lire le cpu_id depuis l'offset 0x6028
    "mov edi, [0x6028]",    // cpu_id (argument 1)
    "mov esi, [0x602C]",    // lapic_id (argument 2)
    "mov rdx, rsp",         // kernel_stack_top (argument 3)

    // Aligner la pile sur 16 octets
    "and rsp, -16",

    // Appeler ap_entry (symbol défini dans smp/init.rs)
    "call ap_entry",

    // Ne doit jamais retourner — halt si ça arrive
    "2: hlt",
    "jmp 2b",

    // ── GDT temporaire (partagée boot) ─────────────────────────────────────
    ".balign 8",
    ".global trampoline_gdt",
    "trampoline_gdt:",
    ".quad 0",              // null
    ".quad 0x00AF9A000000FFFF", // KERNEL_CS64 : Long mode, executable
    ".quad 0x00CF92000000FFFF", // KERNEL_DS32 : data 32 bits

    ".global trampoline_gdtr",
    "trampoline_gdtr:",
    ".word 23",             // limit = 3 * 8 - 1
    ".long 0x6050 + 8",     // base = adresse de trampoline_gdt

    ".global trampoline_end",
    "trampoline_end:",

    ".size trampoline_start, trampoline_end - trampoline_start",
    ".previous",            // retour à .text
);

// Exports linker
extern "C" {
    static trampoline_start: u8;
    static trampoline_end:   u8;
}

// ── Installation du trampoline ────────────────────────────────────────────────

/// Copie le trampoline à 0x6000 et configure les pointeurs PML4
///
/// Appelé par `early_init` avant `smp_boot_aps()`.
pub fn install_trampoline() {
    // SAFETY: trampoline_start/end sont définis par le linker dans .trampoline
    let src: *const u8 = unsafe { &trampoline_start };
    let end: *const u8 = unsafe { &trampoline_end };
    let size = unsafe { end.offset_from(src) as usize };

    let dst = TRAMPOLINE_PHYS as *mut u8;

    // SAFETY: 0x6000 est dans le Low Memory libre, identity-mappé
    unsafe {
        core::ptr::copy_nonoverlapping(src, dst, size);
    }

    // Écrire le PML4 courant dans l'offset 0x20 du trampoline
    let pml4_phys: u64;
    // SAFETY: lecture CR3 — adresse physique du PML4 courant
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) pml4_phys, options(nostack, nomem)); }
    let pml4_slot = (TRAMPOLINE_PHYS + 0x20) as *mut u64;
    // SAFETY: offset dans le trampoline copié
    unsafe { core::ptr::write_volatile(pml4_slot, pml4_phys & 0xFFFF_FFFF_F000); }

    // Écrire le GDTR pointant vers trampoline_gdt (offset 0x50)
    // La GDT temporaire est déjà dans le trampoline copié
}
