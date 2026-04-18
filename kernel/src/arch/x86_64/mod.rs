//! # arch/x86_64 — Implémentation architecture x86_64
//!
//! Point d'entrée de toute la logique spécifique à x86_64.
//! Exporte les primitives utilisées par les couches supérieures.


pub mod acpi;
pub mod apic;
pub mod boot;
pub mod cpu;
pub mod exceptions;
pub mod gdt;
pub mod idt;
pub mod irq;
pub mod memory_iface;
pub mod paging;
pub mod sched_iface;   // Pont FFI arch → scheduler (C ABI exports)
pub mod smp;
pub mod spectre;
pub mod syscall;
pub mod time;
pub mod tss;
pub mod vga_early;
pub mod virt;

use core::sync::atomic::AtomicBool;

use super::ArchInfo;

// ── Constantes globales ───────────────────────────────────────────────────────

/// Taille de page standard (4 KiB)
pub const PAGE_SIZE: usize = 4096;

/// Adresse de base du noyau en espace virtuel
pub const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Limite supérieure de la mémoire physique addressable (48 bits PA)
pub const MAX_PHYS_ADDR: u64 = (1u64 << 48) - 1;

/// Niveaux de page tables (PML4 → PDPT → PD → PT)
pub const PAGE_TABLE_LEVELS: usize = 4;

// ── État global de l'architecture ────────────────────────────────────────────

#[allow(dead_code)]
static ARCH_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Collecte les informations d'architecture post-init
pub fn arch_info() -> ArchInfo {
    ArchInfo {
        cpu_count:  smp::percpu::cpu_count(),
        has_apic:   cpu::features::CPU_FEATURES.has_apic(),
        has_x2apic: cpu::features::CPU_FEATURES.has_x2apic(),
        has_acpi:   acpi::parser::acpi_available(),
        page_size:  PAGE_SIZE,
    }
}

/// Arrête le CPU courant (halt irréversible)
///
/// # SAFETY
/// Appelé uniquement depuis le kernel panic handler ou idle loop.
#[inline(never)]
pub fn halt_cpu() -> ! {
    loop {
        // SAFETY: cli+hlt atomique; IF=0 donc jamais réveillé — boucle infinie intentionnelle.
        unsafe {
            core::arch::asm!(
                "cli",
                "hlt",
                options(nomem, nostack)
            );
        }
    }
}

/// Retard spin-loop courte (en nanosecondes approximatif)
#[inline(always)]
pub fn spin_delay_cycles(cycles: u64) {
    let start = cpu::tsc::read_tsc();
    while cpu::tsc::read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// Barrière mémoire complète (mfence)
#[inline(always)]
pub fn memory_barrier() {
    // SAFETY: instruction architecturale sans effet de bord dangereux
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}

/// Barrière lecture (lfence)
#[inline(always)]
pub fn load_fence() {
    // SAFETY: idem
    unsafe {
        core::arch::asm!("lfence", options(nostack, preserves_flags));
    }
}

/// Barrière écriture (sfence)
#[inline(always)]
pub fn store_fence() {
    // SAFETY: idem
    unsafe {
        core::arch::asm!("sfence", options(nostack, preserves_flags));
    }
}

/// Invalide une ligne TLB pour l'adresse virtuelle donnée
#[inline(always)]
pub fn invlpg(virt_addr: u64) {
    // SAFETY: invlpg est sûr — ne fait qu'invalider une entrée TLB locale
    unsafe {
        core::arch::asm!(
            "invlpg [{addr}]",
            addr = in(reg) virt_addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Lit CR3 (Physical address de PML4 + flags PCID)
#[inline(always)]
pub fn read_cr3() -> u64 {
    let val: u64;
    // SAFETY: lecture de CR3 — privilège requis Ring 0 garanti par contexte noyau
    unsafe {
        core::arch::asm!("mov {val}, cr3", val = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Écrit CR3 (déclenche TLB flush si PCID != 0 et bit 63 = 0)
///
/// # SAFETY
/// L'appelant doit garantir que `cr3_val` pointe vers une PML4 valide
/// et que l'adresse de retour reste mappée dans le nouveau table.
#[inline(always)]
pub unsafe fn write_cr3(cr3_val: u64) {
    // SAFETY: vérification déléguée à l'appelant
    unsafe {
        core::arch::asm!("mov cr3, {val}", val = in(reg) cr3_val, options(nostack, nomem));
    }
}

/// Lit CR2 (adresse de faute page)
#[inline(always)]
pub fn read_cr2() -> u64 {
    let val: u64;
    // SAFETY: lecture registre de contrôle — aucun effet de bord
    unsafe {
        core::arch::asm!("mov {val}, cr2", val = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Lit CR4 (flags de contrôle avancés)
#[inline(always)]
pub fn read_cr4() -> u64 {
    let val: u64;
    // SAFETY: lecture registre de contrôle
    unsafe {
        core::arch::asm!("mov {val}, cr4", val = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Écrit CR4
///
/// # SAFETY
/// L'appelant est responsable de ne pas désactiver des protections critiques
/// (SMEP, SMAP, PCIDE, etc.) sans les avoir correctement désactivées avant.
#[inline(always)]
pub unsafe fn write_cr4(cr4_val: u64) {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!("mov cr4, {val}", val = in(reg) cr4_val, options(nostack, nomem));
    }
}

/// Active les interruptions (sti)
///
/// # SAFETY
/// L'appelant doit s'assurer que IDT et pile IST sont correctement configurés.
#[inline(always)]
pub unsafe fn enable_interrupts() {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!("sti", options(nostack, nomem));
    }
}

/// Désactive les interruptions (cli)
#[inline(always)]
pub fn disable_interrupts() {
    // SAFETY: cli est sûr en Ring 0 — ne fait que modifier IF dans RFLAGS
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }
}

/// Lit RFLAGS
#[inline(always)]
pub fn read_rflags() -> u64 {
    let flags: u64;
    // SAFETY: lecture registre d'état — aucun effet de bord
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {flags}",
            flags = out(reg) flags,
            options(nostack)
        );
    }
    flags
}

/// Section critique : désactive les interruptions et retourne l'état RFLAGS précédent
#[must_use]
#[inline(always)]
#[cfg(all(test, not(target_os = "none")))]
pub fn irq_save() -> u64 {
    0
}

/// Section critique : désactive les interruptions et retourne l'état RFLAGS précédent
#[must_use]
#[inline(always)]
#[cfg(not(all(test, not(target_os = "none"))))]
pub fn irq_save() -> u64 {
    let flags = read_rflags();
    disable_interrupts();
    flags
}

/// Restaure l'état d'interruption depuis des RFLAGS sauvegardés
#[inline(always)]
#[cfg(all(test, not(target_os = "none")))]
pub fn irq_restore(_flags: u64) {}

/// Restaure l'état d'interruption depuis des RFLAGS sauvegardés
#[inline(always)]
#[cfg(not(all(test, not(target_os = "none"))))]
pub fn irq_restore(flags: u64) {
    // SAFETY: restauration d'un état RFLAGS précédemment sauvegardé — sûr
    unsafe {
        core::arch::asm!(
            "push {flags}",
            "popfq",
            flags = in(reg) flags,
            options(nostack)
        );
    }
}

/// Effectue un OUT byte vers un port I/O
///
/// # SAFETY
/// L'appelant garantit que `port` est un port I/O valide et que
/// l'écriture à cet instant est cohérente avec le driver.
#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nostack, nomem)
        );
    }
}

/// Effectue un IN byte depuis un port I/O
///
/// # SAFETY
/// L'appelant garantit que `port` est un port I/O valide.
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nostack, nomem)
        );
    }
    val
}

/// OUT dword vers port I/O
///
/// # SAFETY
/// Idem outb.
#[inline(always)]
pub unsafe fn outl(port: u16, val: u32) {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") val,
            options(nostack, nomem)
        );
    }
}

/// IN dword depuis port I/O
///
/// # SAFETY
/// Idem inb.
#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx") port,
            out("eax") val,
            options(nostack, nomem)
        );
    }
    val
}

/// Délai I/O (~1-4µs) — écriture sur port 0x80 (port debug POST)
#[inline(always)]
pub fn io_delay() {
    // SAFETY: port 0x80 est le port debug BIOS/POST — écriture sans effet
    unsafe { outb(0x80, 0x00); }
}
