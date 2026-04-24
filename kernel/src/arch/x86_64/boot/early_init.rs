//! # arch/x86_64/boot/early_init.rs — Initialisation architecture précoce
//!
//! Premier code Rust exécuté après le passage 64 bits du bootloader.
//! Cette fonction est appelée depuis le point d'entrée ASM (`_start` ou `kernel_start`).
//!
//! ## Invariants d'entrée
//! - Mode long 64 bits actif
//! - Paging identité (1:1) sur les 4 premiers GiB (ou selon Multiboot2/UEFI)
//! - GDT temporaire du bootloader chargée
//! - IDT vide (ou absente)
//! - Interruptions désactivées (EFLAGS.IF = 0)
//!
//! ## Ce que fait early_init
//! 1. Détecter les fonctionnalités CPU
//! 2. Charger la GDT per-CPU BSP
//! 3. Charger l'IDT
//! 4. Initier TSS + IST stacks
//! 5. Init per-CPU / GS
//! 6. Initialiser le TSC
//! 7. Détection hyperviseur
//! 8. Init ACPI (RSDP → MADT → HPET)
//! 9. Init APIC
//! 10. Init FPU / SSE / AVX
//! 11. Appliquer les mitigations Spectre/Meltdown
//! 12. Boot des APs (SMP)
//! 13. Retourner → kernel_main()

// ── Pile de boot du BSP ───────────────────────────────────────────────────────

// La pile de boot BSP (64 KiB) est définie dans main.rs via global_asm! sous
// la section .boot_stack (type @nobits, non stockée dans l'image).
// Le symbole `_exo_boot_stack_top` est le sommet (adresse de fin) de la pile ;
// c'est l'adresse chargée dans RSP par `_start` au tout début du boot.

extern "C" {
    /// Sommet de la pile de boot BSP (adresse de fin + 1 de la section .boot_stack).
    /// Défini par le global_asm! dans main.rs.
    static _exo_boot_stack_top: u8;
}

/// Retourne l'adresse du sommet de la pile de boot BSP.
///
/// Utilisé pour initialiser RSP0 dans la TSS avant le premier task switch.
pub fn boot_stack_top() -> u64 {
    // &raw const est stable depuis Rust 1.82 et ne requière pas unsafe.
    // _exo_boot_stack_top est défini dans main.rs (global_asm!),
    // jamais nul, valide pour toute la durée de vie du kernel.
    &raw const _exo_boot_stack_top as u64
}

// ── Informations de boot passées au kernel principal ──────────────────────────

/// Informations de boot rassemblées par early_init
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BootFramebufferFormat {
    Rgbx = 0,
    Bgrx = 1,
    Unknown = 2,
    None = 0xFFFF_FFFF,
}

/// Informations de boot rassemblées par early_init
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub rsdp_phys: u64,
    pub multiboot2_magic: u32,
    pub multiboot2_addr: u64,
    pub uefi_memmap_addr: u64,
    pub uefi_memmap_size: u64,
    pub uefi_desc_size: u32,
    pub total_memory_kb: u64,
    pub cpu_count: u32,
    pub lapic_base: u64,
    pub framebuffer_phys_addr: u64,
    pub framebuffer_width: u32,
    pub framebuffer_height: u32,
    pub framebuffer_stride: u32,
    pub framebuffer_bpp: u32,
    pub framebuffer_format: BootFramebufferFormat,
    pub framebuffer_size_bytes: u64,
}

impl BootInfo {
    const fn zeroed() -> Self {
        Self {
            rsdp_phys: 0,
            multiboot2_magic: 0,
            multiboot2_addr: 0,
            uefi_memmap_addr: 0,
            uefi_memmap_size: 0,
            uefi_desc_size: 0,
            total_memory_kb: 0,
            cpu_count: 1,
            lapic_base: 0xFEE00000,
            framebuffer_phys_addr: 0,
            framebuffer_width: 0,
            framebuffer_height: 0,
            framebuffer_stride: 0,
            framebuffer_bpp: 0,
            framebuffer_format: BootFramebufferFormat::None,
            framebuffer_size_bytes: 0,
        }
    }
}

// ── Point d'entrée principal de l'initialisation arch ────────────────────────

/// Initialisation complète de l'architecture x86_64
///
/// Appelée depuis `kernel_start` (entrée ASM) avec :
/// - `mb2_magic` : magic Multiboot2 (0x36d76289) ou 0 si UEFI
/// - `mb2_info`  : pointeur vers la structure Multiboot2 (ou 0)
/// - `rsdp_phys` : adresse physique RSDP (passée par le bootloader, 0 = auto-scan)
///
/// # Safety
/// Préconditions : Ring 0, interruptions off, mode long 64 bits.
pub unsafe fn arch_boot_init(mb2_magic: u32, mb2_info: u64, rsdp_phys: u64) -> BootInfo {
    // Sonde de debug inline pour chaque étape (port 0xE9)
    macro_rules! probe {
        ($b:expr) => {
            core::arch::asm!("out 0xe9, al", in("al") $b as u8, options(nostack, nomem));
        }
    }

    // ── Étape 1 : Détecter fonctionnalités CPU ────────────────────────────────
    probe!(b'1');
    super::super::cpu::features::init_cpu_features();
    let features = super::super::cpu::features::cpu_features();

    // Assertions critiques (init_cpu_features() en fait déjà certaines)
    assert!(features.has_sse2(), "SSE2 requis");
    assert!(features.has_syscall(), "SYSCALL requis");

    // ── Étape 2 : GDT per-CPU BSP ────────────────────────────────────────────
    probe!(b'2');
    let kernel_stack_top = boot_stack_top();
    super::super::gdt::init_gdt_for_cpu(0, kernel_stack_top);

    // ── Étape 3 : IDT ─────────────────────────────────────────────────────────
    probe!(b'3');
    super::super::idt::init_idt();
    super::super::idt::load_idt();

    // ── Étape 4 : TSS per-CPU BSP (IST stacks) ────────────────────────────────
    // (déjà fait dans init_gdt_for_cpu qui appelle init_tss_for_cpu + load_tss)

    // ── Étape 5 : Per-CPU data / GS ───────────────────────────────────────────
    probe!(b'5');
    let lapic_id_bsp = 0; // BSP est CPU 0 ; LAPIC ID sera mis à jour après init APIC
    super::super::smp::percpu::init_percpu_for_bsp(kernel_stack_top, lapic_id_bsp);

    // ── Étape 6 : TSC ──────────────────────────────────────────────────────────
    probe!(b'6');
    super::super::cpu::tsc::init_tsc(0);

    // ── Étape 7 : FPU / SSE / AVX ────────────────────────────────────────────
    probe!(b'7');
    super::super::cpu::fpu::init_fpu_for_cpu();

    // ── Étape 8 : Détection hyperviseur ──────────────────────────────────────
    probe!(b'8');
    let hv = super::super::virt::detect::detect_hypervisor();
    let _ = hv;

    // ── Étape 9 : ACPI ────────────────────────────────────────────────────────
    probe!(b'9');
    let mut boot_info = BootInfo::zeroed();

    let rsdp = if rsdp_phys != 0 {
        super::super::acpi::parser::init_acpi_from_rsdp(rsdp_phys);
        rsdp_phys
    } else {
        match super::super::acpi::parser::find_rsdp() {
            Some(addr) => {
                super::super::acpi::parser::init_acpi_from_rsdp(addr);
                addr
            }
            None => 0,
        }
    };
    boot_info.rsdp_phys = rsdp;

    let acpi = super::super::acpi::parser::acpi_info();

    // MADT → LAPIC IDs + I/O APIC
    let madt_info = if acpi.madt_phys != 0 {
        Some(super::super::acpi::madt::parse_madt(acpi.madt_phys))
    } else {
        None
    };

    // HPET
    if acpi.hpet_phys != 0 {
        super::super::acpi::hpet::init_hpet(acpi.hpet_phys);
    }

    // PM Timer
    if acpi.fadt_phys != 0 {
        super::super::acpi::pm_timer::init_pm_timer(acpi.fadt_phys);
    }

    // ── Étape 10 : APIC ──────────────────────────────────────────────────────
    probe!(b'a');
    super::super::apic::init_apic_system();
    if acpi.madt_phys != 0 {
        super::super::apic::io_apic::init_all_ioapics();
    }

    // Recalibrer le timer LAPIC après le TSC
    probe!(b'b');
    super::super::apic::local_apic::calibrate_lapic_timer();

    // ── Intégration arch ↔ memory : enregistrement sender IPI TLB ──────────────
    probe!(b'c');
    super::super::memory_iface::init_memory_integration();

    // ── Étape 11 : Init SYSCALL interface ────────────────────────────────────
    probe!(b'd');
    super::super::syscall::init_syscall();

    // ── Étape 12 : Multiboot2 / UEFI ─────────────────────────────────────────
    probe!(b'f');
    if mb2_magic == 0x36d76289 && mb2_info != 0 {
        boot_info.multiboot2_magic = mb2_magic;
        boot_info.multiboot2_addr = mb2_info;
        let mb2 = super::multiboot2::parse_multiboot2(mb2_info);
        boot_info.total_memory_kb = mb2.total_memory_kb;
        boot_info.rsdp_phys = if mb2.rsdp_phys != 0 {
            mb2.rsdp_phys
        } else {
            rsdp
        };
        boot_info.framebuffer_phys_addr = mb2.framebuffer_phys_addr;
        boot_info.framebuffer_width = mb2.framebuffer_width;
        boot_info.framebuffer_height = mb2.framebuffer_height;
        boot_info.framebuffer_stride = mb2.framebuffer_stride;
        boot_info.framebuffer_bpp = mb2.framebuffer_bpp;
        boot_info.framebuffer_format = match mb2.framebuffer_format {
            super::multiboot2::MultibootFramebufferFormat::None => BootFramebufferFormat::None,
            super::multiboot2::MultibootFramebufferFormat::Rgbx => BootFramebufferFormat::Rgbx,
            super::multiboot2::MultibootFramebufferFormat::Bgrx => BootFramebufferFormat::Bgrx,
            super::multiboot2::MultibootFramebufferFormat::Unknown => {
                BootFramebufferFormat::Unknown
            }
        };
        boot_info.framebuffer_size_bytes = mb2.framebuffer_size_bytes;

        // ── Init sous-système mémoire physique (E820) ──────────────────────────────────
        // Règle MEM-02 DOC2 : EmergencyPool EN PREMIER avant tout allocateur
        crate::memory::physical::frame::emergency_pool::init();
        // Phases 1→4 : bitmap | free_regions | slab | NUMA (depuis carte E820)
        super::memory_map::init_memory_subsystem_multiboot2(&mb2);

        // ── Init espace d'adressage kernel ──────────────────────────────────────
        // Enregistre la PML4 courante (déjà configurée par le bootloader)
        crate::memory::virt::address_space::KERNEL_AS.init(
            crate::memory::core::types::PhysAddr::new(super::super::read_cr3()),
        );

        // ── Protections mémoire hardware (NX / SMEP / SMAP / PKU) ──────────────
        // Activées après l'init complète du sous-système mémoire (DOC2 §2.3)
        crate::memory::protection::init();
    } else if mb2_magic == super::memory_map::EXOBOOT_MAGIC_U32 && mb2_info != 0 {
        // ── Chemin exo-boot UEFI ─────────────────────────────────────────────
        // `mb2_info` = adresse physique du BootInfo exo-boot (identité-mappée).
        // `mb2_magic` = EXOBOOT_MAGIC_U32 (0x4F4F_5845 "EXOO").
        //
        // RÈGLE MEM-02 : EmergencyPool EN PREMIER.
        crate::memory::physical::frame::emergency_pool::init();

        // Init sous-système mémoire depuis la carte mémoire exo-boot
        super::memory_map::init_memory_subsystem_exoboot(mb2_info);

        // Lire RSDP du BootInfo (offset 6200)
        let rsdp_from_bi = core::ptr::read_volatile((mb2_info + 6200) as *const u64);
        if rsdp_from_bi != 0 {
            super::super::acpi::parser::init_acpi_from_rsdp(rsdp_from_bi);
            boot_info.rsdp_phys = rsdp_from_bi;
        } else if rsdp != 0 {
            // Utilise l'adresse passée en paramètre (rsdp_phys = RDX = 0 depuis _start,
            // mais arch_boot_init peut avoir fait un scan ACPI avant ce bloc)
            boot_info.rsdp_phys = rsdp;
        }

        // Layout exo-boot synchronisé avec docs/exo-boot/BOOTINFO.md :
        // 6160..6199 = FramebufferInfo repr(C).
        boot_info.framebuffer_phys_addr = core::ptr::read_volatile((mb2_info + 6160) as *const u64);
        boot_info.framebuffer_width = core::ptr::read_volatile((mb2_info + 6168) as *const u32);
        boot_info.framebuffer_height = core::ptr::read_volatile((mb2_info + 6172) as *const u32);
        boot_info.framebuffer_stride = core::ptr::read_volatile((mb2_info + 6176) as *const u32);
        boot_info.framebuffer_bpp = core::ptr::read_volatile((mb2_info + 6180) as *const u32);
        boot_info.framebuffer_format =
            match core::ptr::read_volatile((mb2_info + 6184) as *const u32) {
                0 => BootFramebufferFormat::Rgbx,
                1 => BootFramebufferFormat::Bgrx,
                0xFFFF_FFFF => BootFramebufferFormat::None,
                _ => BootFramebufferFormat::Unknown,
            };
        boot_info.framebuffer_size_bytes =
            core::ptr::read_volatile((mb2_info + 6192) as *const u64);

        // Enregistre la PML4 courante (configurée par exo-boot)
        crate::memory::virt::address_space::KERNEL_AS.init(
            crate::memory::core::types::PhysAddr::new(super::super::read_cr3()),
        );

        // Protections mémoire hardware (NX / SMEP / SMAP)
        crate::memory::protection::init();
    } else {
        panic!(
            "unsupported boot protocol: magic={:#x} info={:#x}",
            mb2_magic, mb2_info
        );
    }

    // ── Étape 12b : Mitigations Spectre/Meltdown ─────────────────────────────
    // KPTI peut construire des shadow page tables et allouer des frames ; il faut
    // donc attendre que le sous-système mémoire et la PML4 kernel soient prêts.
    probe!(b'e');
    super::super::spectre::apply_mitigations_bsp();

    // ── Étape 13b : Security init (SECURITY_READY) ─────────────────────────
    // Libère les APs du spin-wait BOOT-SEC dans smp/init.rs.
    // L'appel est idempotent côté boot flow: kernel_init() vérifie aussi ce flag.
    probe!(b'h');
    if !crate::security::is_security_ready() {
        let kaslr_entropy =
            super::super::cpu::tsc::read_tsc() ^ ((mb2_magic as u64) << 32) ^ mb2_info ^ rsdp;
        crate::security::security_init(
            kaslr_entropy,
            crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR,
        );
    }

    // ── Étape 14 : SMP — boot des APs ────────────────────────────────────────
    probe!(b'g');
    if let Some(ref madt) = madt_info {
        if madt.cpu_count > 1 {
            super::trampoline_asm::install_trampoline();
            let bsp_lapic = super::super::apic::local_apic::lapic_id();
            super::super::smp::init::smp_boot_aps(madt, bsp_lapic as u32);
        }
    }
    boot_info.cpu_count = super::super::smp::init::smp_cpu_count();
    assert!(
        boot_info.cpu_count as usize <= 256,
        "Trop de CPUs pour MAX_CPUS"
    );

    probe!(b'Z'); // arch_boot_init terminé
    boot_info
}
