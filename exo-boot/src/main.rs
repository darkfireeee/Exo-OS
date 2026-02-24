//! main.rs  Point d'entrée du bootloader Exo-OS.
//!
//! CE FICHIER est le pivot entre les deux chemins de démarrage :
//!   - UEFI  (x86_64-unknown-uefi)   `efi_main()`  appelé par le firmware
//!   - BIOS  (x86_64-unknown-none)   `exoboot_main_bios()` appelé par stage2.asm
//!
//! Séquence d'initialisation UEFI (DOC10/BOOT-*) :
//!   1.  Initialise uefi-services (allocateur, logger minimal)
//!   2.  Lit la config exo-boot.cfg via EFI_FILE_PROTOCOL (FAT32/ESP)
//!   3.  Initialise le framebuffer GOP (affichage logo + barre de progression)
//!   4.  Lit la carte mémoire UEFI pour préparer BootInfo
//!   5.  Charge et vérifie la signature Ed25519 du kernel ELF
//!   6.  Récupère 64 octets d'entropie via EFI_RNG_PROTOCOL (KASLR + CSPRNG)
//!   7.  Parse l'ELF, alloue la mémoire kernel, charge les segments, applique PIE
//!   8.  Alloue et construit les tables de pages initiales
//!   9.  Construit BootInfo (contrat formel bootloaderkernel, RÈGLE BOOT-03)
//!  10.  ExitBootServices  POINT DE NON-RETOUR (RÈGLE BOOT-06)
//!  11.  Flush TLB, barrières mémoire
//!  12.  Transfère le contrôle au kernel _start

#![no_std]
#![no_main]
#![feature(allocator_api)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::missing_safety_doc)]

//  Modules 

pub mod bios;
pub mod config;
pub mod display;
pub mod kernel_loader;
pub mod memory;
// Le panic handler : en mode UEFI, uefi_services le fournit.
// Ce module est compilé dans les deux cas pour les fonctions utilitaires d'affichage.
pub mod panic;

#[cfg(feature = "uefi-boot")]
pub mod uefi;

//  CHEMIN UEFI 

#[cfg(feature = "uefi-boot")]
use ::uefi::entry;
#[cfg(feature = "uefi-boot")]
use ::uefi::{Handle, Status};
#[cfg(feature = "uefi-boot")]
use ::uefi::table::{Boot, SystemTable};

/// Point d'entrée UEFI — signature imposée par la spec UEFI 2.x.
#[cfg(feature = "uefi-boot")]
#[entry]
fn efi_main(
    image_handle: Handle,
    mut system_table: SystemTable<Boot>,
) -> Status {
    // Étape 1 : Initialisation uefi-services (allocateur global + logger ConOut)
    uefi_services::init(&mut system_table).expect("uefi-services init impossible");

    let boot_services = system_table.boot_services();

    // Étape 2 : Configuration
    let cfg = config::load_config_uefi(boot_services, image_handle);

    // Étape 3 : Framebuffer GOP
    let framebuffer = uefi::protocols::graphics::init_gop(boot_services)
        .unwrap_or_else(|_| display::framebuffer::Framebuffer::absent());
    display::init_display_from_gop(
        framebuffer.phys_addr, framebuffer.width, framebuffer.height,
        framebuffer.stride, framebuffer.format, framebuffer.size_bytes,
    );
    boot_println!("Exo-Boot v0.1.0  UEFI {}x{}", framebuffer.width, framebuffer.height);

    // Étape 4 : Carte mémoire UEFI
    let uefi_memmap = memory::map::collect_uefi_memory_map(boot_services)
        .expect("Impossible de récupérer la carte mémoire UEFI");
    boot_println!("Carte mémoire: {} entrées", uefi_memmap.entries.len());

    // Étape 5 : Chargement kernel + vérification signature (RÈGLE BOOT-02)
    let kernel_data = uefi::protocols::file::load_file(
        boot_services, image_handle, cfg.kernel_path.as_str(),
    ).expect("Impossible de charger le kernel depuis l'ESP");
    boot_println!("Kernel: {} bytes", kernel_data.len());
    kernel_loader::verify::verify_kernel_or_panic(kernel_data.as_bytes());
    boot_println!("Signature OK");

    // Étape 6 : Entropie (RÈGLE BOOT-05 : 64 bytes)
    let entropy = uefi::protocols::rng::collect_entropy(boot_services, 64)
        .expect("EFI_RNG_PROTOCOL indisponible");

    // Étape 7 : Allocation + chargement ELF + relocations
    let kernel_phys_dest = {
        use ::uefi::table::boot::{AllocateType, MemoryType};
        let elf = kernel_loader::elf::ElfKernel::parse(kernel_data.as_bytes())
            .expect("Kernel ELF64 invalide");
        let load_pages = ((elf.load_size() + 0xFFF) / 0x1000 + 1) as usize;
        boot_services
            .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, load_pages)
            .expect("Impossible d'allouer la mémoire kernel")
    };

    let params = kernel_loader::KernelLoadParams {
        elf_data:      kernel_data.as_bytes(),
        elf_phys_addr: kernel_data.phys_addr(),
        entropy,
        kaslr_enabled: cfg.kaslr_enabled,
        secure_boot:   cfg.secure_boot_required,
    };
    // SAFETY : kernel_phys_dest est alloué, accessible et identité-mappé par UEFI.
    let load_result = unsafe {
        kernel_loader::load_kernel(&params, kernel_phys_dest)
    }.expect("Erreur chargement kernel ELF");
    boot_println!("Kernel mappé: base={:#x} entry={:#x}", load_result.phys_base, load_result.entry_phys);

    // Étape 8 : Allocation du pool de tables de pages (AVANT ExitBootServices)
    let page_table_pool = memory::paging::allocate_page_table_pool(boot_services)
        .expect("Impossible d'allouer le pool de tables de pages");

    // Étape 9 : Construction de BootInfo
    let (_mem_key, mem_map) = memory::map::convert_uefi_memory_map(uefi_memmap);

    // Cherche le RSDP ACPI AVANT ExitBootServices (SystemTable encore valide)
    let acpi_rsdp_val = memory::regions::find_acpi_rsdp_uefi(&system_table).unwrap_or(0);

    static mut BOOT_INFO: core::mem::MaybeUninit<kernel_loader::handoff::BootInfo> =
        core::mem::MaybeUninit::uninit();
    // SAFETY : bootloader single-threaded, BOOT_INFO initialisé ici uniquement.
    // On écrit via le pointeur brut pour éviter &mut sur static mutable.
    let boot_info_ref = unsafe {
        let ptr = core::ptr::addr_of_mut!(BOOT_INFO);
        (*ptr).write(kernel_loader::handoff::BootInfo::new())
    };
    boot_info_ref.set_memory_regions(mem_map.regions_slice());
    boot_info_ref.framebuffer          = display::get_boot_info_framebuffer();
    boot_info_ref.acpi_rsdp            = acpi_rsdp_val;
    boot_info_ref.entropy              = entropy;
    boot_info_ref.kernel_physical_base = load_result.phys_base;
    boot_info_ref.kernel_entry_offset  = load_result.entry_offset;
    boot_info_ref.kernel_elf_phys      = params.elf_phys_addr;
    boot_info_ref.kernel_elf_size      = kernel_data.len() as u64;
    boot_info_ref.boot_flags = {
        use kernel_loader::handoff::boot_flags::*;
        let mut flags = UEFI_BOOT;
        if cfg.kaslr_enabled                       { flags |= KASLR_ENABLED; }
        if cfg.secure_boot_required                { flags |= SECURE_BOOT_ACTIVE; }
        if boot_info_ref.framebuffer.is_present()  { flags |= FRAMEBUFFER_PRESENT; }
        if boot_info_ref.acpi_rsdp != 0            { flags |= ACPI2_PRESENT; }
        flags
    };
    boot_info_ref.record_tsc();
    boot_println!("BootInfo: {} régions mémoire", boot_info_ref.memory_region_count);

    // Étape 10 : ExitBootServices  POINT DE NON-RETOUR (RÈGLE BOOT-06)
    let (_runtime_st, _) = system_table.exit_boot_services(::uefi::table::boot::MemoryType::LOADER_DATA);
    crate::uefi::exit::mark_boot_services_exited();

    // Étape 10b : Construction des tables de pages (APRÈS ExitBootServices)
    // Utilise le pool pré-alloué  pas de BootServices nécessaires.
    let page_tables = memory::paging::setup_kernel_page_tables(page_table_pool)
        .expect("Impossible de construire les tables de pages");

    // Étape 11 : Flush TLB + barrières mémoire
    unsafe {
        core::arch::asm!("mov rax, cr3", "mov cr3, rax", out("rax") _, options(nomem, nostack));
        core::arch::asm!("mfence", options(nomem, nostack));
    }

    // Étape 12 : Handoff vers le kernel  ne retourne jamais
    unsafe {
        kernel_loader::handoff::handoff_to_kernel(
            boot_info_ref as *const kernel_loader::handoff::BootInfo,
            load_result.entry_phys,
            load_result.phys_base,
            &page_tables,
        )
    }
}

//  CHEMIN BIOS 

/// Point d'entrée BIOS  appelé par stage2.asm après passage en mode long.
///
/// SAFETY : La stack, GDT et mode d'exécution sont garantis par stage2.asm.
#[cfg(feature = "bios-boot")]
#[no_mangle]
pub unsafe extern "C" fn exoboot_main_bios(
    e820_buffer_addr: u64,
    e820_entry_count: u32,
) -> ! {
    let cfg = config::load_config_bios();

    let mut vga = bios::vga::VgaWriter::new(bios::vga::Color::White, bios::vga::Color::Black);
    let _ = vga.write_str("Exo-Boot [BIOS] v0.1.0\n");

    let mem_map = memory::map::collect_bios_memory_map(e820_buffer_addr, e820_entry_count)
        .expect("Carte mémoire E820 invalide");

    // Le kernel ELF a été chargé par stage2.asm en mémoire shadow à 2 MiB.
    // On lit directement depuis cette zone physique.
    const STAGE2_DISK_SHADOW_BASE: u64 = 0x0020_0000; // 2 MiB
    const KERNEL_MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MiB
    // SAFETY : stage2.asm garantit que les données kernel sont présentes ici.
    let kernel_data: &[u8] = unsafe {
        core::slice::from_raw_parts(STAGE2_DISK_SHADOW_BASE as *const u8, KERNEL_MAX_BYTES)
    };

    kernel_loader::verify::verify_kernel_or_panic(kernel_data);

    let entropy = bios::collect_entropy_bios();

    let bios_params = kernel_loader::KernelLoadParams {
        elf_data:      kernel_data,
        elf_phys_addr: STAGE2_DISK_SHADOW_BASE,
        entropy,
        kaslr_enabled: cfg.kaslr_enabled,
        secure_boot:   cfg.secure_boot_required,
    };

    let load_result = unsafe {
        kernel_loader::load_kernel(&bios_params, 0)
    }.expect("Erreur chargement kernel");

    let page_tables = memory::paging::setup_kernel_page_tables(
        memory::paging::BIOS_PAGE_TABLE_POOL,
    ).expect("Impossible de construire les tables de pages");

    static mut BOOT_INFO_BIOS: kernel_loader::handoff::BootInfo =
        kernel_loader::handoff::BootInfo::ZEROED;
    let boot_info_ref = unsafe { &mut BOOT_INFO_BIOS };
    *boot_info_ref = kernel_loader::handoff::BootInfo::new();
    boot_info_ref.set_memory_regions(mem_map.regions_slice());
    boot_info_ref.framebuffer          = kernel_loader::handoff::FramebufferInfo::absent();
    boot_info_ref.acpi_rsdp            = memory::regions::find_acpi_rsdp_bios();
    boot_info_ref.entropy              = entropy;
    boot_info_ref.kernel_physical_base = load_result.phys_base;
    boot_info_ref.kernel_entry_offset  = load_result.entry_offset;
    boot_info_ref.kernel_elf_phys      = bios_params.elf_phys_addr;
    boot_info_ref.kernel_elf_size      = KERNEL_MAX_BYTES as u64;
    boot_info_ref.boot_flags           = 0;
    boot_info_ref.record_tsc();

    let _ = vga.write_str("Handoff kernel...\n");

    unsafe {
        kernel_loader::handoff::handoff_to_kernel(
            boot_info_ref as *const kernel_loader::handoff::BootInfo,
            load_result.entry_phys,
            load_result.phys_base,
            &page_tables,
        )
    }
}
