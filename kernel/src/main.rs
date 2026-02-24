//! # main.rs — Point d'entrée kernel Exo-OS
//!
//! Flux de boot :
//! ```
//! GRUB (Multiboot2) / UEFI stub
//!   → _start  (ASM — mode long déjà actif via GRUB, ou activé ici)
//!     → empile mb2_magic, mb2_info, rsdp_phys
//!     → call kernel_main
//!       → arch_boot_init()   (arch : CPU, GDT, IDT, TSS, APIC, SMP…)
//!       → kernel_init()      (memory → scheduler → process → ipc → fs)
//!       → halt_cpu()         (idle loop, scheduler prend le relais)
//! ```

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(dead_code)]
#![allow(unused_variables)]

use exo_os_kernel as kernel;

// ── Header Multiboot2 ─────────────────────────────────────────────────────────
// GRUB cherche le magic 0xE85250D6 dans les 32 premiers KiB de l'image.
// Doit être dans la section `.multiboot2`, qui est la PREMIÈRE dans linker.ld.

core::arch::global_asm!(
    ".section .multiboot2, \"a\"",
    ".align 8",
    // Magic number Multiboot2
    ".long 0xE85250D6",
    // Architecture : 0 = i386 (protected mode — GRUB active le 64 bits après)
    ".long 0",
    // Longueur du header (16 bytes minimum — magic + arch + length + checksum)
    ".long 16",
    // Checksum : -(magic + arch + length) doit valoir 0 mod 2^32
    // = -(0xE85250D6 + 0 + 16) = -(0xE85250E6) mod 2^32 = 0x17ADAF1A
    // Note: LLVM assembler n'accepte pas les expressions négatives dans .long
    ".long 0x17ADAF1A",
    // End tag (type=0, flags=0, size=8)
    ".short 0", ".short 0", ".long 8",
);

// ── Entrée assembleur _start ──────────────────────────────────────────────────
// GRUB charge le kernel en mode protégé 32 bits puis, si le header Multiboot2
// contient le tag "entry address 64 bits", saute directement en mode long.
// Pour simplifier, on suppose que la transition 64 bits est déjà faite
// (ce qui est le cas avec GRUB2 + grub.cfg `multiboot2`).
//
// Registres à l'entrée Multiboot2 :
//   EAX = 0x36d76289  (magic)
//   EBX = adresse physique de la structure Multiboot2 Info
//
// On pousse ces valeurs + rsdp_phys=0 puis on appelle kernel_main.

core::arch::global_asm!(
    ".section .text.boot",
    ".global _start",
    ".type _start, @function",
    "_start:",

    // Désactiver les interruptions — on est en mode long mais sans GDT/IDT valide
    "cli",

    // Charger la pile de boot (définie dans early_init.rs::BOOT_STACK)
    // Le symbole _exo_boot_stack_top est résolu par le linker via early_init.rs
    "lea rsp, [rip + _exo_boot_stack_top]",

    // Aligner la pile sur 16 bytes (ABI System V)
    "and rsp, -16",

    // Arguments de kernel_main (System V ABI — registres) :
    //   rdi = mb2_magic  (u32)  — fourni par GRUB dans EAX/RAX
    //   rsi = mb2_info   (u64)  — fourni par GRUB dans EBX/RBX (adresse 32 bits)
    //   rdx = rsdp_phys  (u64)  — 0 par défaut (arch_boot_init le cherchera)
    "mov edi, eax",          // mb2_magic (GRUB met le magic dans EAX)
    "mov rsi, rbx",          // mb2_info  (GRUB met l'adresse info dans EBX)
    "xor edx, edx",           // rsdp_phys = 0 (on cherchera en ACPI)

    // Appeler le point d'entrée Rust
    "call kernel_main",

    // Ne doit jamais être atteint — GRUB reprend la main si kernel_main retourne
    "cli",
    "0: hlt",
    "jmp 0b",

    ".size _start, . - _start",
);

// ── Point d'entrée Rust principal ─────────────────────────────────────────────

/// `kernel_main` — première fonction Rust exécutée sur le BSP.
///
/// # Safety
/// - Appelé depuis `_start` (ASM) une unique fois sur le BSP
/// - Interruptions désactivées (EFLAGS.IF = 0)
/// - Pile BSP valide (`BOOT_STACK` dans `early_init.rs`)
/// - Mode long 64 bits actif, GDT minimale bootloader chargée
#[no_mangle]
pub unsafe extern "C" fn kernel_main(
    mb2_magic: u32,   // 0x36d76289 si boot Multiboot2 valide, sinon 0
    mb2_info:  u64,   // Adresse physique de la structure Multiboot2 Info (ou 0)
    rsdp_phys: u64,   // Adresse physique RSDP passée par le bootloader (ou 0)
) -> ! {
    // ── Phase 1 : Architecture (GDT, IDT, TSS, per-CPU, TSC, FPU, ACPI, APIC,
    //              SYSCALL, Spectre, SMP boot des APs)
    // SAFETY: arch_boot_init doit être le PREMIER code Rust exécuté en Ring 0.
    // Préconditions satisfaites : mode long, interruptions off, pile valide.
    let _boot_info = kernel::arch_boot_init(mb2_magic, mb2_info, rsdp_phys);

    // ── Phases 2–7 : Init des couches (memory → scheduler → process → ipc → fs)
    // SAFETY: kernel_init suit l'ordre strict des couches défini en docs/refonte.
    // Doit être appelé après arch_boot_init (nécessite GDT + interruptions prêtes).
    kernel::kernel_init();

    // ── Idle loop ─────────────────────────────────────────────────────────────
    // Une fois le scheduler démarré, le APIC timer tick (vecteur 0x20) interrompra
    // ce HLT et le scheduler planifiera l'idle task à la place de cette boucle.
    kernel::halt_cpu()
}

// ── Gestionnaires panic / OOM ─────────────────────────────────────────────────
// Définis dans lib.rs — le binaire hérite de l'implémentation via le rlib.
// Ne pas redéfinir ici pour éviter les conflits de lang_item.
