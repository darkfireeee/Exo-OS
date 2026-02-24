# exo-boot — Bootloader Exo-OS

> **Version** : 0.1.0  
> **Crate** : `exo-boot` (`no_std`, workspace séparé)  
> **Cibles** : `x86_64-unknown-uefi` · `x86_64-unknown-none` (BIOS)  
> **Licence** : MIT

---

## Vue d'ensemble

`exo-boot` est le bootloader de première couche d'Exo-OS. Il est responsable de :

1. **Initialiser le matériel minimal** avant de passer la main au kernel
2. **Charger et vérifier** l'image ELF du kernel depuis l'ESP (UEFI) ou le disque (BIOS)
3. **Construire `BootInfo`** — le contrat formel transmis au kernel
4. **Configurer les tables de pages initiales** (identité + higher-half)
5. **Exécuter le handoff** vers `_start` du kernel

Il supporte deux chemins de démarrage indépendants :

| Chemin | Feature | Cible Cargo | Firmware |
|--------|---------|-------------|----------|
| **UEFI** | `uefi-boot` | `x86_64-unknown-uefi` | Machines modernes (2013+) |
| **BIOS** | `bios-boot` | `x86_64-unknown-none` | Machines legacy, VMs |

---

## Fonctionnalités clés

| Fonctionnalité | Description |
|----------------|-------------|
| **Secure Boot (BOOT-02)** | Vérification signature Ed25519 du kernel avant tout chargement |
| **KASLR (BOOT-07)** | Randomisation de l'adresse de chargement kernel à partir d'entropie hardware |
| **GOP Framebuffer** | Affichage boot progress, logo, messages de diagnostic |
| **BootInfo (BOOT-03)** | Structure `#[repr(C)]` transmise au kernel : mémoire, framebuffer, ACPI, entropie |
| **ExitBootServices (BOOT-06)** | Gestion propre du point de non-retour UEFI |
| **Mémoire unifiée (BOOT-04)** | Fusion UEFI Memory Map + E820 en format commun kernel |
| **ACPI RSDP (BOOT-04)** | Localisation depuis les EFI Config Tables ou scan BIOS 0xE0000–0xFFFFF |

---

## Structure du dépôt

```
exo-boot/
├── Cargo.toml                   # Crate no_std
├── build.rs                     # Sélection linker script selon cible
├── linker/
│   ├── uefi.ld                  # Linker script (commentaire — lld-link auto)
│   └── bios.ld                  # Linker script GNU ld (BIOS flat ELF)
└── src/
    ├── main.rs                  # Points d'entrée : efi_main() + exoboot_main_bios()
    ├── panic.rs                 # Panic handler (BIOS uniquement — UEFI : uefi-services)
    ├── bios/                    # Chemin BIOS legacy
    │   ├── mod.rs               # Détection CPU (CPUID), RDRAND
    │   ├── mbr.asm              # MBR 512 bytes (stage 1)
    │   ├── stage2.asm           # Stage 2 : A20, mode protégé → long
    │   ├── vga.rs               # Sortie texte VGA 80×25
    │   └── disk.rs              # Lecture disque INT 13h EDD (LBA 48-bit)
    ├── config/                  # Configuration runtime
    │   └── mod.rs               # Parseur exo-boot.cfg
    ├── display/                 # Affichage bootloader
    │   ├── mod.rs               # boot_print!/boot_println! + barre de progression
    │   └── framebuffer.rs       # Abstraction framebuffer GOP (pixel writes)
    ├── kernel_loader/           # Chargement kernel ELF
    │   ├── mod.rs               # Orchestration : parse → verify → load → relocate
    │   ├── elf.rs               # Parseur ELF64 (PT_LOAD, PT_DYNAMIC)
    │   ├── verify.rs            # Vérification Ed25519 + SHA-512
    │   ├── relocations.rs       # KASLR + relocations R_X86_64_RELATIVE
    │   └── handoff.rs           # BootInfo + saut kernel _start
    ├── memory/                  # Gestion mémoire bootloader
    │   ├── mod.rs               # Constantes globales (PAGE_SIZE, etc.)
    │   ├── map.rs               # MemoryRegion, MemoryKind, collecte UEFI map
    │   ├── paging.rs            # Tables de pages PML4 (identity + higher-half)
    │   └── regions.rs           # ACPI RSDP + régions spéciales
    └── uefi/                    # Couche UEFI
        ├── entry.rs             # Validation préconditions EFI
        ├── services.rs          # Wrappers Boot Services (alloc, memmap, free)
        ├── secure_boot.rs       # Lecture variables EFI SecureBoot/SetupMode
        ├── exit.rs              # ExitBootServices + flag BOOT_SERVICES_ACTIVE
        └── protocols/
            ├── graphics.rs      # GOP — Graphics Output Protocol
            ├── file.rs          # EFI_FILE_PROTOCOL — lecture FAT32/ESP
            ├── loaded_image.rs  # EFI_LOADED_IMAGE — device handle bootloader
            └── rng.rs           # EFI_RNG_PROTOCOL — entropie hardware
```

---

## Documentation détaillée

| Document | Contenu |
|----------|---------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Vue d'ensemble, modules, interactions |
| [BOOT_SEQUENCE.md](BOOT_SEQUENCE.md) | Séquence de démarrage pas-à-pas UEFI et BIOS |
| [BOOTINFO.md](BOOTINFO.md) | Contrat `BootInfo` bootloader → kernel |
| [MODULES.md](MODULES.md) | Référence de chaque module Rust |
| [SECURITY.md](SECURITY.md) | Secure Boot, Ed25519, KASLR |
| [MEMORY.md](MEMORY.md) | Carte mémoire, tables de pages, régions |
| [BUILD.md](BUILD.md) | Compilation, features, dépendances |

---

## Compilation rapide

```powershell
# Chemin UEFI (défaut)
cd exo-boot
cargo build --target x86_64-unknown-uefi \
  --features "uefi-boot,dev-skip-sig" \
  -Z build-std=core,alloc,compiler_builtins

# Résultat
# target/x86_64-unknown-uefi/debug/exo-boot.efi
```

Voir [BUILD.md](BUILD.md) pour les options complètes.

---

## Règles architecturales respectées

| Règle | Description |
|-------|-------------|
| **BOOT-02** | Signature Ed25519 vérifiée AVANT tout chargement mémoire du kernel |
| **BOOT-03** | `BootInfo` entièrement initialisé, champs réservés à zéro |
| **BOOT-04** | Chaque région mémoire identifiée au type exact (11 types distincts) |
| **BOOT-05** | 64 bytes d'entropie hardware collectés via EFI_RNG_PROTOCOL |
| **BOOT-06** | ExitBootServices = point de non-retour géré par `BOOT_SERVICES_ACTIVE` |
| **BOOT-07** | KASLR : base aléatoire alignée 2 MiB, relocations PIE appliquées |

---

## Synchronisation kernel

Le contrat `BootInfo` est partagé entre exo-boot et le kernel. Tout changement
dans `kernel_loader/handoff.rs` doit être synchronisé avec :

```
kernel/src/arch/x86_64/boot/early_init.rs
kernel/src/arch/x86_64/boot/memory_map.rs
```

Magic de synchronisation : `BOOT_INFO_MAGIC = 0x4F42_5F53_4F4F_5845` ("EXOOS_BO" LE).
