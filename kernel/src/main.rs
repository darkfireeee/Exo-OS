//! # main.rs — Point d'entrée kernel Exo-OS
//!
//! Flux de boot (GRUB Multiboot2 BIOS) :
//! ```
//!   GRUB → _start  (32-bit protected mode, EAX=magic, EBX=info)
//!     → trampoline 32→64 bits (page tables + EFER.LME + CR0.PG + retf)
//!     → _start64  (64-bit long mode)
//!       → call kernel_main(mb2_magic, mb2_info, 0)
//!         → arch_boot_init()  (CPU, GDT, IDT, TSS, APIC, SMP…)
//!         → kernel_init()     (memory → scheduler → process → ipc → fs)
//!         → halt_cpu()
//! ```
//!
//! Flux de boot (exo-boot UEFI) :
//! ```
//!   exo-boot → handoff_to_kernel() → met EAX=EXOBOOT_MAGIC_U32, RBX=BootInfo
//!     → jump vers _start (32-bit compat, mais continue identiquement)
//!     → même trampoline → kernel_main(EXOBOOT_MAGIC_U32, boot_info_phys, 0)
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
    ".long 0xE85250D6",          // Magic Multiboot2
    ".long 0",                   // Architecture i386 (GRUB entre en 32-bit prot. mode)
    ".long 24",                  // Header length : 16 (fixe) + 8 (end tag)
    ".long 0x17ADAF12",          // Checksum : -(0xE85250D6 + 0 + 24) mod 2^32
    ".short 0", ".short 0", ".long 8",  // End tag
);

// ── Tables de pages de boot (BSS — zéro-initialisées, alignées 4 KiB) ─────────
// Utilisées par le trampoline 32→64 bits pour mettre en place le mapping
// identité 0..1 GiB avant d'activer la pagination (long mode exige des PTEs).
//
// Layout :
//   PML4[0]    → _boot_pdpt (identity: virtual 0..512 GiB)
//   PDPT[0]    → _boot_pd   (identity: virtual 0..1 GiB)
//   PDPT[3]    → _boot_pd_high (MMIO APIC: virtual 3..4 GiB, pour LAPIC/IOAPIC)
//   PD[0..512] = huge pages 2 MiB (identity: virtual 0..1 GiB → phys 0..1 GiB)
//   _boot_pd_high[502] = 0xFEC00000 (IOAPIC MMIO)
//   _boot_pd_high[503] = 0xFEE00000 (LAPIC MMIO)
//
// NOTE : 1 GiB couvre largement le kernel (~10 MiB) + stack (~46 MiB).
//        Le mapping APIC (0xFEE00000) est nécessaire pour init_apic_system.
core::arch::global_asm!(
    ".section .bss",
    ".balign 4096",
    ".global _boot_pml4",          "_boot_pml4:",          ".space 4096",
    ".global _boot_pdpt",          "_boot_pdpt:",          ".space 4096",
    ".global _boot_pd",            "_boot_pd:",            ".space 4096",
    ".global _boot_pd_high",       "_boot_pd_high:",       ".space 4096",
    // Sauvegarde des args Multiboot2 (EAX/EBX) pendant la transition 32→64
    ".global _mb2_saved_magic", "_mb2_saved_magic:", ".long 0",
    ".global _mb2_saved_info",  "_mb2_saved_info:",  ".quad 0",
);

// ── GDT 64-bit minimale pour le trampoline de boot ────────────────────────────
// Chargée en 32-bit par lgdt, puis utilisée en 64-bit après le retf.
// Format : null (8B) + code64 (8B) + data64 (8B) = 24 bytes.
//
//   CS = 0x08 : L=1 (64-bit code), DPL=0, P=1, Type=0xA (code exec/read)
//   DS = 0x10 : L=0, D=1 (32/64-bit data), DPL=0, P=1, Type=0x2 (data r/w)
core::arch::global_asm!(
    ".section .rodata",
    ".balign 16",
    ".global _boot_gdt",
    "_boot_gdt:",
    ".quad 0x0000000000000000",   // 0x00 — null descriptor
    ".quad 0x00af9a000000ffff",   // 0x08 — 64-bit code, DPL=0, L=1
    ".quad 0x00cf92000000ffff",   // 0x10 — data,       DPL=0, D=1
    "_boot_gdt_end_marker:",
    ".global _boot_gdtr",
    "_boot_gdtr:",
    ".short 23",                  // limit = 3*8 - 1 = 23
    ".long  _boot_gdt",           // base  (32-bit physique — kernel < 4 GiB)
);

// ── Trampoline de boot : 32-bit → 64-bit (long mode) ─────────────────────────
//
// GRUB entre ici en mode protégé 32-bit :
//   EAX = 0x36d76289 (Multiboot2 magic)
//   EBX = adresse physique de la structure Multiboot2 Info (<4 GiB)
//   CS  = descripteur flat 32-bit (base=0, limit=4GiB)
//   PE=1, IF=0, paging=0
//
// Objectif : avant d'appeler kernel_main (64-bit Rust), activer le long mode.
//
// SAFETY : toutes les adresses de symboles sont < 1 GiB (kernel à 1 MiB).
//          L'identité-mapping couvre 0..1 GiB en huge pages 2 MiB.
core::arch::global_asm!(
    ".section .text.boot",
    ".global _start",
    ".type _start, @function",
    ".code32",

    // ─────────────────────────────────────────────────────────────────────────
    "_start:",

    "cli",
    "cld",

    // ── Sauvegarder args Multiboot2 avant de clobber EBX ─────────────────────
    "mov dword ptr [_mb2_saved_magic], eax",
    "mov dword ptr [_mb2_saved_info],  ebx",
    "mov dword ptr [_mb2_saved_info + 4], 0",      // zero-extend à 64 bits

    // ── Stack 32-bit provisoire (adresse physique = virtuelle avant paging) ───
    // _exo_boot_stack_top est défini plus bas (section .boot_stack).
    // lea donne l'ADRESSE effective du label (et non son contenu).
    "lea esp, [_exo_boot_stack_top]",
    "and esp, -16",

    // ── Construire le PD : 512 huge pages 2 MiB (identity 0..1 GiB) ──────────
    // Flags : P=1 (present), R/W=1 (writable), PS=1 (2 MiB page) = 0x83
    // Chaque entrée : (index << 21) | 0x83
    "xor ecx, ecx",
    "_pd_loop:",
    "  cmp ecx, 512",
    "  jge _pd_done",
    "  mov eax, ecx",
    "  shl eax, 21",            // adresse physique = index × 2 MiB
    "  or  eax, 0x83",          // flags R/W + Present + PageSize
    "  lea edi, [_boot_pd]",    // base du PD
    "  mov ebx, ecx",
    "  shl ebx, 3",             // offset = index × 8 octets
    "  add edi, ebx",
    "  mov dword ptr [edi],     eax",
    "  mov dword ptr [edi + 4], 0",    // upper 32 bits = 0
    "  inc ecx",
    "  jmp _pd_loop",
    "_pd_done:",

    // ── PDPT[0] → PD (identity 0..1 GiB) ────────────────────────────────────
    "lea eax, [_boot_pd]",
    "or  eax, 0x03",            // P + R/W
    "mov dword ptr [_boot_pdpt],     eax",
    "mov dword ptr [_boot_pdpt + 4], 0",

    // ── _boot_pd_high : mapping MMIO APIC (LAPIC + IOAPIC) dans 3..4 GiB ────
    // LAPIC MMIO : 0xFEE00000 → PD_high[503] (PDPT[3], PD entry 503)
    // IOAPIC MMIO: 0xFEC00000 → PD_high[501] (PD entry 501... wait: compute below)
    // Calcul : PD_high entry index = (phys - 0xC0000000) >> 21
    //  - IOAPIC 0xFEC00000 : (0xFEC00000 - 0xC0000000) >> 21 = 0x3EC00000 >> 21 = 502
    //  - LAPIC  0xFEE00000 : (0xFEE00000 - 0xC0000000) >> 21 = 0x3EE00000 >> 21 = 503
    // Entry format (2 MiB huge page) : phys_base | 0x83 (P + R/W + PS)
    //   IOAPIC page base: 503 - 1 = 502 → phys = 0xC0000000 + 502*0x200000 = 0xFEC00000
    //   LAPIC  page base: 503        → phys = 0xC0000000 + 503*0x200000 = 0xFEE00000
    //   Note : 0xFEC00000 | 0x83 = 0xFEC00083 (lower 32 bits; upper = 0)
    //          0xFEE00000 | 0x83 = 0xFEE00083

    // Écrire PD_high[502] = 0xFEC0009B (IOAPIC 2 MiB page, P+R/W+PS+PWT+PCD → UC)
    // 0x9B = 0x83 | 0x08 (PWT) | 0x10 (PCD) : sélectionne PAT[3]=UC (défaut Intel)
    "lea edi, [_boot_pd_high]",
    "add edi, 4016",
    "mov dword ptr [edi],     0xFEC0009B",
    "mov dword ptr [edi + 4], 0",

    // Écrire PD_high[503] = 0xFEE0009B (LAPIC 2 MiB page, P+R/W+PS+PWT+PCD → UC)
    "lea edi, [_boot_pd_high]",
    "add edi, 4024",
    "mov dword ptr [edi],     0xFEE0009B",
    "mov dword ptr [edi + 4], 0",

    // ── PDPT[3] → _boot_pd_high (MMIO APIC region) ; offset = 3×8 = 24 ───────
    "lea eax, [_boot_pd_high]",
    "or  eax, 0x03",            // P + R/W
    "mov dword ptr [_boot_pdpt + 24],     eax",
    "mov dword ptr [_boot_pdpt + 28], 0",

    // ── PML4[256] → _boot_pdpt (physmap 0xFFFF_8000_0000_0000) ──────────────────
    // Réutilise _boot_pdpt existant (2 MiB pages via _boot_pd).
    // CPUID.PDPE1GB non requis — pages 1 GiB non supportées universellement.
    // Couvre 0..1 GiB physique via PDPT[0]→_boot_pd — suffisant pour -m 256M.
    // PML4 index 256 = bits[47:39] de 0xFFFF_8000_0000_0000 ; offset = 2048.
    "lea eax, [_boot_pdpt]",
    "or  eax, 0x03",            // P + R/W
    "mov dword ptr [_boot_pml4 + 2048],     eax",
    "mov dword ptr [_boot_pml4 + 2052], 0",

    // ── PML4[0] → PDPT ────────────────────────────────────────────────────────
    "lea eax, [_boot_pdpt]",
    "or  eax, 0x03",            // P + R/W
    "mov dword ptr [_boot_pml4],     eax",
    "mov dword ptr [_boot_pml4 + 4], 0",

    // ── Charger notre GDT 64-bit ──────────────────────────────────────────────
    "lgdt [_boot_gdtr]",

    // ── CR3 = PML4 ────────────────────────────────────────────────────────────
    "lea eax, [_boot_pml4]",
    "mov cr3, eax",

    // ── CR4.PAE = 1 (Physical Address Extension, requis pour long mode) ───────
    "mov eax, cr4",
    "or  eax, 0x00000020",      // bit 5 = PAE
    "mov cr4, eax",

    // ── EFER.LME = 1 (Long Mode Enable, MSR 0xC0000080) ─────────────────────
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or  eax, 0x00000100",      // bit 8 = LME
    "wrmsr",

    // ── CR0.PG = 1 : active la pagination → active le long mode ──────────────
    // Le CPU passe immédiatement en mode compatibilité (32-bit submode de LM).
    // Le far jump ci-dessous passe en mode 64-bit pur (CS avec L=1).
    "mov eax, cr0",
    "or  eax, 0x80000000",      // bit 31 = PG
    "mov cr0, eax",

    // ── Saut lointain vers CS=0x08 (64-bit) : entre en mode 64-bit ───────────
    // Technique RETF : empile (CS, EIP) en ordre inverse puis `retf`.
    // `retf` dépile EIP puis CS → charge CS=0x08 (64-bit code descriptor).
    // Après : CPU en mode 64-bit, CS=0x08.
    "lea eax, [_start64]",      // EAX = adresse physique de _start64
    "push dword ptr 0x08",      // empile CS = 0x08 (64-bit code)
    "push eax",                 // empile EIP = adresse de _start64
    "retf",                     // far return → CS:EIP chargés → mode 64-bit

    // ── Halt (ne devrait jamais être atteint) ─────────────────────────────────
    "_boot_halt32:",
    "cli",
    "hlt",
    "jmp _boot_halt32",

    // ─────────────────────────────────────────────────────────────────────────
    // _start64 : premier code 64-bit après transition long mode
    // ─────────────────────────────────────────────────────────────────────────
    ".code64",
    ".global _start64",
    ".type _start64, @function",
    "_start64:",

    // Marqueur debug port 0xE9 : 'X' = 0x58 → confirme que _start64 est atteint
    "mov al, 0x58",         // 'X'
    "out 0xe9, al",

    // Recharger les sélecteurs de données avec DS=0x10 (data 64-bit)
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "xor ax, ax",
    "mov fs, ax",
    "mov gs, ax",

    // Stack 64-bit propre (symbole résolu par le linker)
    "lea rsp, [rip + _exo_boot_stack_top]",
    "and rsp, -16",

    // Restaurer les arguments Multiboot2 sauvegardés dans .bss
    //   RDI = mb2_magic (u32)
    //   RSI = mb2_info  (u64)
    //   RDX = rsdp_phys (u64, 0 → arch_boot_init scannera ACPI)
    "mov edi, dword ptr [rip + _mb2_saved_magic]",
    "mov rsi, qword ptr [rip + _mb2_saved_info]",
    "xor edx, edx",

    // Appeler kernel_main (Rust, 64-bit SysV ABI)
    "call kernel_main",

    // Idle définitif si kernel_main retourne (ne devrait pas)
    "cli",
    "_start64_halt:",
    "hlt",
    "jmp _start64_halt",

    ".size _start, . - _start",
);

// ── Pile de boot BSP ──────────────────────────────────────────────────────────
// Section .boot_stack : NOBITS (comme .bss, absent de l'image ELF).
// `_exo_boot_stack_top` = RSP initial du BSP (pile croît vers le bas).
core::arch::global_asm!(
    ".section .boot_stack, \"aw\", @nobits",
    ".balign 4096",
    ".global _exo_boot_stack_bottom",
    "_exo_boot_stack_bottom:",
    ".space 65536",          // 64 KiB de pile boot
    ".global _exo_boot_stack_top",
    "_exo_boot_stack_top:",
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
    // Debug : marqueur de début kernel_main sur port 0xE9 ('K' = 0x4B)
    core::arch::asm!("mov al, 0x4B", "out 0xe9, al", options(nostack, nomem));

    // ── Phase 1 : Architecture (GDT, IDT, TSS, per-CPU, TSC, FPU, ACPI, APIC,
    //              SYSCALL, Spectre, SMP boot des APs)
    // SAFETY: arch_boot_init doit être le PREMIER code Rust exécuté en Ring 0.
    // Préconditions satisfaites : mode long, interruptions off, pile valide.
    let _boot_info = kernel::arch_boot_init(mb2_magic, mb2_info, rsdp_phys);

    // Debug : arch_boot_init terminé ('A' = 0x41)
    core::arch::asm!("mov al, 0x41", "out 0xe9, al", options(nostack, nomem));

    // ── Phases 2–7 : Init des couches (memory → scheduler → process → ipc → fs)
    // SAFETY: kernel_init suit l'ordre strict des couches défini en docs/refonte.
    // Doit être appelé après arch_boot_init (nécessite GDT + interruptions prêtes).
    kernel::kernel_init();

    // Debug : kernel_init terminé ('I' = 0x49)
    core::arch::asm!("mov al, 0x49", "out 0xe9, al", options(nostack, nomem));

    // Debug : boot complet → '\n', 'O', 'K', '\n'
    core::arch::asm!(
        "mov al, 0x0a", "out 0xe9, al",
        "mov al, 0x4f", "out 0xe9, al",  // 'O'
        "mov al, 0x4b", "out 0xe9, al",  // 'K' (second, diff from first K)
        "mov al, 0x0a", "out 0xe9, al",
        options(nostack, nomem)
    );

    // ── Idle loop ─────────────────────────────────────────────────────────────
    // Une fois le scheduler démarré, le APIC timer tick (vecteur 0x20) interrompra
    // ce HLT et le scheduler planifiera l'idle task à la place de cette boucle.
    kernel::halt_cpu()
}

// ── Gestionnaires panic / OOM ─────────────────────────────────────────────────
// Définis dans lib.rs — le binaire hérite de l'implémentation via le rlib.
// Ne pas redéfinir ici pour éviter les conflits de lang_item.
