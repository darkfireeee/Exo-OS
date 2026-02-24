# Séquence de démarrage exo-boot

## Chemin UEFI (x86_64-unknown-uefi)

### Vue séquentielle complète

```
Firmware UEFI POST
       │
       │  Vérifie signature PE32+ du bootloader (Secure Boot firmware)
       │  Charge exo-boot.efi dans LOADER_CODE memory
       │
       ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 0 : EFI Entry                                  │
│  efi_main(image_handle, system_table)                 │
│                                                        │
│  uefi_services::init(&mut system_table)               │
│    → Allocateur global (uefi pool)                    │
│    → Logger ConOut minimal                            │
│    → #[panic_handler] fourni par uefi-services        │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 1 : Validation préconditions                   │
│  uefi/entry::validate_uefi_entry_preconditions()      │
│                                                        │
│  ✓ Version UEFI ≥ 2.0 (GOP + RNG requis)             │
│  ✓ Secure Boot status (SecureBoot, SetupMode)         │
│  ✓ ConOut disponible (non bloquant si absent)         │
│                                                        │
│  Produit EntryDiagnostics (version, SB flags, etc.)  │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 2 : Configuration                              │
│  config::load_config_uefi(bt, image_handle)           │
│                                                        │
│  Ouvre EFI_LOADED_IMAGE → device handle               │
│  Ouvre SimpleFileSystem sur ce device                 │
│  Lit \EFI\EXOOS\exo-boot.cfg (FAT32/ESP)             │
│  Parse les paires clé=valeur                          │
│                                                        │
│  Si absent → BootConfig::default_config() silencieux  │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 3 : Framebuffer GOP                            │
│  uefi/protocols/graphics::init_gop(bt)                │
│                                                        │
│  Localise EFI_GRAPHICS_OUTPUT_PROTOCOL                │
│  Sélectionne le mode natif (résolution max)           │
│  Lit : phys_addr, width, height, stride, format       │
│                                                        │
│  display::init_display_from_gop(...)                  │
│    → clear() : remplissage fond sombre                │
│    → draw_boot_logo() : logo ASCII                    │
│    → Enregistre dans GLOBAL_FRAMEBUFFER               │
│                                                        │
│  Si absent → Framebuffer::absent() (affichage ConOut) │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 4 : Carte mémoire UEFI                         │
│  memory/map::collect_uefi_memory_map(bt)              │
│                                                        │
│  bt.memory_map_size() → taille hint                   │
│  bt.allocate_pool(LOADER_DATA, size + 8KB)            │
│  bt.memory_map(buffer) → MemoryMap + MemoryMapKey    │
│                                                        │
│  Compression : MemoryDescriptor (40B) →               │
│               UefiMemoryDescriptorCompact (24B)       │
│  Stockage dans ArrayVec<_, 1024> (pas de heap)        │
│                                                        │
│  ⚠ La clé devient invalide à chaque allocate/free   │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 5 : Chargement kernel (RÈGLE BOOT-02)          │
│  uefi/protocols/file::load_file(bt, handle, path)     │
│                                                        │
│  EFI_FILE_PROTOCOL → open(\EFI\EXOOS\kernel.elf)     │
│  FileInfo → taille fichier                            │
│  bt.allocate_pool(LOADER_DATA, file_size)             │
│  Lecture complète dans buffer                         │
│                                                        │
│  kernel_loader/verify::verify_kernel_or_panic()       │
│    → Localise section .kernel_sig (256 bytes fin ELF) │
│    → Vérifie marqueur "EXOSIG01"                      │
│    → [secure-boot] Ed25519::verify(pubkey, hash, sig) │
│    → [dev-skip-sig] warn uniquement                   │
│    → PANIC immédiat si invalide                       │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 6 : Entropie hardware (RÈGLE BOOT-05)          │
│  uefi/protocols/rng::collect_entropy(bt, 64)          │
│                                                        │
│  Localise EFI_RNG_PROTOCOL                            │
│  RNG→GetRNG(EFI_RNG_ALGORITHM_RAW, 64, buf)          │
│  Si indisponible → fallback RDRAND + RDSEED + TSC     │
│                                                        │
│  Résultat : [u8; 64] transmis dans BootInfo.entropy   │
│  Utilisé par le kernel pour son CSPRNG                │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 7 : Parse ELF + Alloc + Load + Relocations     │
│                                                        │
│  ElfKernel::parse(kernel_data)                        │
│    → Valide magic ELF64, EM_X86_64, ET_DYN/ET_EXEC   │
│    → Compte PT_LOAD segments                          │
│                                                        │
│  KASLR (RÈGLE BOOT-07) :                              │
│    compute_kaslr_base(&entropy) → adresse aléatoire   │
│    Alignée 2 MiB, dans plage [1 GiB, 256 GiB]        │
│                                                        │
│  bt.allocate_pages(AnyPages, LOADER_DATA, n_pages)    │
│                                                        │
│  ElfKernel::load_segments(phys_base)                  │
│    → Par segment PT_LOAD :                            │
│      copy_nonoverlapping(src, dst, filesz)            │
│      write_bytes(dst + filesz, 0, memsz - filesz)    │
│                                                        │
│  apply_pie_relocations(&elf, phys_base)               │
│    → Lit section .dynamic → DT_RELA / DT_RELASZ       │
│    → Pour chaque Elf64Rela :                          │
│      R_X86_64_RELATIVE : *addr = kaslr_base + addend  │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 8 : Tables de pages initiales                  │
│  memory/paging::setup_initial_page_tables(bt, ...)    │
│                                                        │
│  bt.allocate_pages(AnyPages, LOADER_DATA, N)          │
│  Construction PML4 → PDPT → PD (grandes pages 2 MiB) │
│                                                        │
│  Mapping 1 : Identité 0 GiB → 4 GiB                  │
│    PML4[0..3] → PDPT → PD → 2 MiB pages              │
│                                                        │
│  Mapping 2 : Higher-half kernel                       │
│    PML4[511] → PDPT[510..511] → kernel pages          │
│    KERNEL_HIGHER_HALF_BASE = 0xFFFF_FFFF_8000_0000   │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 9 : Construction BootInfo (RÈGLE BOOT-03)      │
│                                                        │
│  BootInfo::new()                                      │
│    magic   = BOOT_INFO_MAGIC ("EXOOS_BO" LE)         │
│    version = BOOT_INFO_VERSION (1)                    │
│                                                        │
│  Remplissage :                                        │
│    memory_regions ← convert_uefi_to_memory_regions() │
│    framebuffer    ← display::get_boot_info_framebuffer│
│    acpi_rsdp      ← find_acpi_rsdp_uefi(&st)         │
│    entropy        ← [u8; 64] collecté à étape 6      │
│    kernel_physical_base ← phys_base après KASLR      │
│    kernel_entry_offset  ← elf.entry_offset()         │
│    boot_flags     ← UEFI_BOOT | KASLR_ENABLED | …   │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 10 : POINT DE NON-RETOUR (RÈGLE BOOT-06)       │
│                                                        │
│  Dernière lecture mémoire map pour clé à jour         │
│  let (runtime_st, _mem_map) =                         │
│      system_table.exit_boot_services(LOADER_DATA)     │
│                                                        │
│  exit::mark_boot_services_exited()                    │
│    → BOOT_SERVICES_ACTIVE.store(false)                │
│                                                        │
│  !! Plus de ConOut, plus d'allocateur, plus de BS !! │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│  ÉTAPE 11 : Handoff vers kernel (RÈGLE BOOT-06/07)   │
│  kernel_loader/handoff::handoff_to_kernel()           │
│                                                        │
│  asm! {                                               │
│    mov cr3, {pml4_phys}   ; Charge nouvelles PTs     │
│    mov rdi, {boot_info}   ; Arg 1 : &BootInfo        │
│    mov rsp, {stack}       ; Nouvelle stack kernel     │
│    jmp {entry_phys}       ; Saut vers kernel _start  │
│  }                                                    │
│                                                        │
│  → Jamais retour (→ !)                                │
└──────────────────────────────────────────────────────┘
```

---

## Chemin BIOS legacy

### MBR → Stage2 → Rust

```
BIOS POST
    │
    ▼  Charge secteur 0 (MBR) à 0x7C00
┌─────────────────────────────────┐
│  bios/mbr.asm  (512 bytes)      │
│                                  │
│  Vérifie boot signature 0xAA55  │
│  Lit stage2 (LBA 1→4) via INT 13h│
│  Saute à 0x7E00                 │
└──────────────────┬──────────────┘
                   │
                   ▼
┌─────────────────────────────────┐
│  bios/stage2.asm                │
│                                  │
│  Active la ligne A20             │
│  Charge GDT (segments 32-bit)   │
│  Passe en mode protégé (CR0.PE) │
│  Charge GDT 64-bit              │
│  Active long mode (EFER.LME)   │
│  Passe en mode long (CR0.PG)   │
│  Saute vers Rust exoboot_main_bios│
└──────────────────┬──────────────┘
                   │
                   ▼
┌─────────────────────────────────┐
│  exoboot_main_bios()            │
│  (même séquence qu'UEFI mais)   │
│                                  │
│  Affichage VGA 80×25            │
│  bios/disk.rs : lecture kernel  │
│    via données pré-chargées     │
│  Même kernel_loader pipeline    │
│  ACPI RSDP : scan 0xE0000 BIOS  │
└─────────────────────────────────┘
```

---

## Gestion d'erreur et panics

| Situation | Comportement |
|-----------|-------------|
| UEFI version < 2.0 | `Err(EntryError::UefiVersionTooOld)` → Status::UNSUPPORTED |
| Kernel absent sur ESP | `FileError::NotFound` → panic avec message |
| Signature invalide | `PANIC` immédiat (BOOT-02 non négociable) |
| GOP absent | `Framebuffer::absent()` → affichage ConOut uniquement |
| RNG indisponible | Fallback RDRAND + RDSEED + TSC (non bloquant) |
| Allocation mémoire échoue | `.expect()` → panic |
| Double ExitBootServices | `debug_assert` dans `mark_boot_services_exited()` |
| BS après exit | `assert_boot_services_active()` → panic |

### Panic pré-ExitBootServices (UEFI)

Le crate `uefi-services` fournit le panic handler. Il affiche via ConOut
puis appelle `EFI RuntimeServices::ResetSystem(EfiResetShutdown)`.

### Panic post-ExitBootServices

Le framebuffer GOP est utilisé directement (`BootWriter`/`PanicWriter`).
ConOut peut ne plus fonctionner — le framebuffer est toujours accessible.

### Panic BIOS

`bios/vga.rs` : écriture directe à 0xB8000, puis `halt_forever()` (CLI + HLT).
