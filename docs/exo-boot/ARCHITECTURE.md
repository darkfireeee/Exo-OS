# Architecture d'exo-boot

## Vue d'ensemble

exo-boot est structuré en **couches indépendantes** qui convergent vers un point unique : le handoff au kernel.

```
┌─────────────────────────────────────────────────────────────────┐
│                         FIRMWARE                                │
│            UEFI (x86_64-unknown-uefi)  │  BIOS legacy           │
└──────────────────────┬──────────────────┬───────────────────────┘
                       │                  │
              ┌────────▼─────┐    ┌───────▼────────┐
              │  efi_main()  │    │ stage2.asm      │
              │  main.rs     │    │ (mode réel→long)│
              └────────┬─────┘    └───────┬─────────┘
                       │                  │
                       └────────┬─────────┘
                                │
              ┌─────────────────▼──────────────────┐
              │           Orchestration              │
              │           main.rs / mod.rs           │
              │                                      │
              │  config → display → memory → loader  │
              └─────────────────┬──────────────────-─┘
                                │
              ┌─────────────────▼──────────────────┐
              │         kernel_loader               │
              │                                     │
              │  verify → elf → relocations → handoff│
              └─────────────────┬───────────────────┘
                                │
              ┌─────────────────▼──────────────────┐
              │            KERNEL _start            │
              │   (RDI = &BootInfo, RSP = stack)    │
              └─────────────────────────────────────┘
```

---

## Modules et responsabilités

### `main.rs` — Pivot central

Contient les **deux points d'entrée** et orchestre la séquence de boot :

- `efi_main(Handle, SystemTable<Boot>)` → chemin UEFI
- `exoboot_main_bios(boot_info: *mut BootInfo)` → chemin BIOS

**Ne contient aucune logique métier** — délègue tout aux sous-modules.

---

### `uefi/` — Couche d'abstraction UEFI

```
uefi/
├── entry.rs          Validation préconditions (version UEFI, ConOut, Secure Boot)
├── services.rs       Wrappers typés des Boot Services EFI
│                       allocate_pages_zeroed()
│                       free_pages()
│                       get_memory_map_raw()
│                       open_protocol_safe<P>()
│                       locate_protocol<P>()
├── secure_boot.rs    Lecture variables EFI (SecureBoot, SetupMode, AuditMode)
├── exit.rs           ExitBootServices + AtomicBool BOOT_SERVICES_ACTIVE
└── protocols/
    ├── graphics.rs   GOP : init, résolution, adresse framebuffer
    ├── file.rs       EFI_FILE_PROTOCOL : load_file(), file_exists()
    ├── loaded_image.rs  EFI_LOADED_IMAGE : device handle du bootloader
    └── rng.rs        EFI_RNG_PROTOCOL : collect_entropy(n_bytes)
```

**Invariant** : Tout appel aux Boot Services passe par `services.rs` qui vérifie
`BOOT_SERVICES_ACTIVE` via `assert_boot_services_active()`.

---

### `bios/` — Chemin BIOS legacy

```
bios/
├── mod.rs      Détection CPU : CPUID (fabricant, RDRAND, TSC), rdrand_entropy()
├── mbr.asm     MBR 512 bytes : lecture stage2 via INT 13h, saut en 32-bit
├── stage2.asm  Gate A20, entrée mode protégé, entrée mode long, appel Rust
├── vga.rs      VgaWriter : écriture texte 80×25 couleurs (MMIO 0xB8000)
└── disk.rs     BiosDisk : lecture secteurs LBA 48-bit via INT 13h EDD
```

**Contrainte** : INT 13h n'est disponible qu'en mode réel. Le code Rust BIOS
travaille uniquement avec les données pré-chargées par `stage2.asm`.

---

### `config/` — Configuration runtime

```
config/
└── mod.rs      load_config_uefi() → BootConfig
                Lit \EFI\EXOOS\exo-boot.cfg (FAT32)
                Parse : kernel_path, kaslr_enabled, secure_boot_required, etc.
                Fallback silencieux sur les valeurs par défaut
```

Chemins par défaut :
- Kernel : `\EFI\EXOOS\kernel.elf`
- Config : `\EFI\EXOOS\exo-boot.cfg`
- Initrd : `\EFI\EXOOS\initrd.img` (optionnel)

---

### `display/` — Affichage bootloader

```
display/
├── mod.rs          boot_print!/boot_println! macros
│                   init_display_from_gop()
│                   update_progress(step, total, msg)
│                   draw_progress_bar()
│                   draw_boot_logo()
└── framebuffer.rs  Framebuffer (struct GOP)
                    BootWriter (impl fmt::Write → framebuffer)
                    PanicWriter (idem, pour le panic handler)
                    try_get_framebuffer() → Option<&Framebuffer>
                    init_global_framebuffer(&Framebuffer)
```

**Statique global** : `GLOBAL_FRAMEBUFFER: Mutex<Option<Framebuffer>>` — accessible
depuis le panic handler même après ExitBootServices.

---

### `memory/` — Gestion mémoire bootloader

```
memory/
├── mod.rs      Constantes : PAGE_SIZE=4096, HUGE_PAGE_SIZE=2MiB
│               MAX_MEMORY_REGIONS=256
│               KERNEL_HIGHER_HALF_BASE=0xFFFF_FFFF_8000_0000
├── map.rs      MemoryKind (11 types), MemoryRegion (base, length, kind)
│               MemoryMap : tableau + méthodes de tri/fusion
│               collect_uefi_memory_map() → RawMemoryMapBuffer
│               build_memory_map_from_uefi() → MemoryMap
├── paging.rs   PageTable, PageTablesSetup
│               setup_initial_page_tables() → PageTablesSetup
│               Mapping identité 0–4 GiB (grandes pages 2 MiB)
│               Mapping higher-half PML4[511] → kernel
└── regions.rs  find_acpi_rsdp_uefi(&SystemTable) → Option<u64>
                find_acpi_rsdp_bios() → Option<u64>
                Scan mémoire UEFI Config Tables ou 0xE0000–0xFFFFF
```

---

### `kernel_loader/` — Chargement kernel

```
kernel_loader/
├── mod.rs          load_kernel(params) → KernelLoadResult
│                   Orchestre : parse → alloc → load_segments → relocate
├── elf.rs          ElfKernel<'a> : parse(), program_headers(), load_segments()
│                   Valide : magic ELF, ET_EXEC/ET_DYN, EM_X86_64, EI_CLASS64
│                   Charge PT_LOAD : file→mem copy + BSS zero-fill
├── verify.rs       verify_kernel_or_panic(&[u8])
│                   KernelSignature (section .kernel_sig, 256 bytes)
│                   Feature secure-boot : Ed25519 + SHA-512
│                   Feature dev-skip-sig : warn mais non-bloquant
├── relocations.rs  apply_pie_relocations(&ElfKernel, phys_base) → Result
│                   compute_kaslr_base(&[u8; 64]) → u64
│                   Parcours .rela.dyn : R_X86_64_RELATIVE + R_X86_64_64
└── handoff.rs      BootInfo (repr(C), align 4096), PixelFormat, FramebufferInfo
                    boot_flags (KASLR_ENABLED, SECURE_BOOT_ACTIVE, …)
                    handoff_to_kernel(phys_entry, &BootInfo) → !
                    kernel_virtual_base(phys_base) → u64
```

---

## Flux de données

```
Firmware UEFI
    │
    ▼
efi_main(image_handle, system_table)
    │
    ├─── config::load_config_uefi()         ──► BootConfig
    │         └── uefi/protocols/file.rs
    │
    ├─── uefi/protocols/graphics::init_gop() ──► Framebuffer
    │         └── display::init_display_from_gop()
    │
    ├─── memory/map::collect_uefi_memory_map() ──► RawMemoryMapBuffer
    │
    ├─── uefi/protocols/file::load_file()  ──► FileBuffer (kernel ELF)
    │
    ├─── kernel_loader/verify::verify_kernel_or_panic()
    │
    ├─── uefi/protocols/rng::collect_entropy(64) ──► [u8; 64]
    │
    ├─── kernel_loader/elf::ElfKernel::parse() ──► ElfKernel
    │         └── boot_services.allocate_pages()
    │         └── ElfKernel::load_segments(phys_base)
    │         └── relocations::apply_pie_relocations()
    │
    ├─── memory/paging::setup_initial_page_tables() ──► PageTablesSetup
    │
    ├─── memory/regions::find_acpi_rsdp_uefi() ──► Option<u64>
    │
    ├─── BootInfo::new() + remplissage
    │
    ├─── system_table.exit_boot_services()   ◄── POINT DE NON-RETOUR
    │         └── exit::mark_boot_services_exited()
    │
    └─── handoff::handoff_to_kernel(entry_phys, &BOOT_INFO)
              └── cr3 ← pml4_phys
              └── jmp kernel_entry
```

---

## Invariants de sécurité

1. **Aucune écriture en mémoire kernel avant `verify_kernel_or_panic()`**
   → Empêche une attaque TOCTOU sur l'image chargée

2. **`BOOT_SERVICES_ACTIVE` protège tous les appels Boot Services**
   → Double protection : atomique + assert dans chaque wrapper

3. **Entropie collectée AVANT KASLR**
   → Garantit que l'entropie transmise au kernel n'est pas contaminée par les allocations KASLR

4. **`BootInfo` align(4096) en static mut**
   → Séparation de page garantie pour éviter les aliasing mémoire

5. **`#![deny(unsafe_op_in_unsafe_fn)]`**
   → Chaque opération unsafe doit être explicitement délimitée avec un bloc `unsafe {}`
