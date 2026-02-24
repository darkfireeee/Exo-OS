# Référence BootInfo

`BootInfo` est la structure de contrat entre exo-boot et le kernel.
Elle est construite entièrement par le bootloader avant `exit_boot_services`,
passée en RDI au kernel et ne doit jamais être modifiée après le handoff.

---

## Constantes de protocole

```rust
/// Magic : "EXOOS_BO" en little-endian (8 octets)
pub const BOOT_INFO_MAGIC: u64 = 0x4F42_5F53_4F4F_5845;

/// Version du format BootInfo
pub const BOOT_INFO_VERSION: u32 = 1;

/// Capacité maximale de la carte mémoire
pub const MAX_MEMORY_REGIONS: usize = 256;

/// Taille de page standard
pub const PAGE_SIZE: u64 = 4096;

/// Taille de grande page (2 MiB)
pub const HUGE_PAGE_SIZE: u64 = 0x200000;

/// Base higher-half du kernel
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_FFFF_8000_0000;
```

---

## Struct BootInfo

```rust
#[repr(C, align(4096))]
pub struct BootInfo {
    // ── Octets 0-11 : Identité ────────────────────────────────────────
    pub magic:               u64,    // doit valoir BOOT_INFO_MAGIC
    pub version:             u32,    // doit valoir BOOT_INFO_VERSION

    // ── Octets 12-15 : Compteur ───────────────────────────────────────
    pub memory_region_count: u32,    // nb régions valides (≤ 256)

    // ── Octets 16 - 16+sizeof(MemoryRegion)*256 : Carte mémoire ─────
    pub memory_regions:      [MemoryRegion; MAX_MEMORY_REGIONS],

    // ── Framebuffer ──────────────────────────────────────────────────
    pub framebuffer:         FramebufferInfo,

    // ── ACPI ─────────────────────────────────────────────────────────
    pub acpi_rsdp:           u64,    // adresse physique RSDP, 0 si absent

    // ── Entropie ─────────────────────────────────────────────────────
    pub entropy:             [u8; 64], // 64 octets CSPRNG seed

    // ── Kernel layout ────────────────────────────────────────────────
    pub kernel_physical_base: u64,   // base physique après KASLR
    pub kernel_entry_offset:  u64,   // offset e_entry depuis base
    pub kernel_elf_phys:      u64,   // adresse physique ELF complet
    pub kernel_elf_size:      u64,   // taille ELF en octets

    // ── Flags et timing ──────────────────────────────────────────────
    pub boot_flags:           u64,   // voir bits ci-dessous
    pub boot_tsc:             u64,   // RDTSC juste avant handoff

    // ── Réservé (extension future) ───────────────────────────────────
    pub _reserved:            [u64; 16],
}
```

### Taille approximative
- `MemoryRegion` = 24 octets × 256 = 6 144 octets
- `FramebufferInfo` ≈ 40 octets
- Champs scalaires ≈ 120 octets
- `_reserved` = 128 octets
- **Total ≈ 6 432 octets** — tient dans deux pages 4 KiB

---

## boot_flags — Définition des bits

| Bit | Masque | Constante | Description |
|-----|--------|-----------|-------------|
| 0 | `0x01` | `KASLR_ENABLED` | KASLR activé : `kernel_physical_base` est aléatoire |
| 1 | `0x02` | `SECURE_BOOT_ACTIVE` | Secure Boot firmware confirmé + signature kernel vérifiée |
| 2 | `0x04` | `UEFI_BOOT` | Démarrage UEFI (sinon BIOS legacy) |
| 3 | `0x08` | `ACPI2_PRESENT` | `acpi_rsdp` pointe une RSDP v2 (XSDP) valide |
| 4 | `0x10` | `FRAMEBUFFER_PRESENT` | `FramebufferInfo` est valide et utilisable |

```rust
// Lecture côté kernel
let is_uefi   = info.boot_flags & 0x04 != 0;
let has_fb    = info.boot_flags & 0x10 != 0;
let has_kaslr = info.boot_flags & 0x01 != 0;
```

---

## Struct FramebufferInfo

```rust
#[repr(C)]
pub struct FramebufferInfo {
    pub phys_addr:  u64,       // adresse physique du framebuffer linéaire
    pub width:      u32,       // résolution horizontale en pixels
    pub height:     u32,       // résolution verticale en pixels
    pub stride:     u32,       // octets par ligne (peut être > width * bpp/8)
    pub bpp:        u32,       // bits par pixel (typiquement 32)
    pub format:     PixelFormat,
    pub size_bytes: u64,       // taille totale en octets (stride * height)
}
```

### PixelFormat

```rust
#[repr(u32)]
pub enum PixelFormat {
    /// Rouge-Vert-Bleu-X : RGBX 8-8-8-8
    Rgbx = 0,
    /// Bleu-Vert-Rouge-X : BGRX 8-8-8-8 (format GOP le plus courant x86)
    Bgrx = 1,
    /// Format non standard (consulter stride/bpp pour calcul manuel)
    Custom = 2,
    /// Pas de framebuffer (FRAMEBUFFER_PRESENT = 0)
    None = 0xFFFF_FFFF,
}
```

---

## Struct MemoryRegion

```rust
#[repr(C)]
pub struct MemoryRegion {
    pub base:  u64,           // adresse physique de début (alignée page 4KiB)
    pub size:  u64,           // taille en octets
    pub kind:  MemoryKind,    // type de la région
    pub _pad:  u32,           // rembourrage pour alignement
}
```

### MemoryKind — Enum complète

| Valeur raw | Variante | Description |
|-----------|---------|-------------|
| `1` | `Usable` | RAM libre, utilisable par le kernel |
| `2` | `KernelCode` | Segments `.text` du kernel (PT_LOAD exécutable) |
| `3` | `KernelData` | Segments `.data`/`.bss` du kernel |
| `4` | `BootloaderReclaimable` | Pages bootloader récupérables après init kernel |
| `5` | `PageTables` | Tables de pages initiales créées par exo-boot |
| `6` | `AcpiReclaimable` | Tables ACPI récupérables après parse |
| `7` | `AcpiNvs` | Mémoire ACPI NVS (persistante) |
| `8` | `Reserved` | Réservé firmware/hardware, ne pas toucher |
| `9` | `Framebuffer` | Région du framebuffer GOP |
| `10` | `Mmio` | Memory-Mapped I/O (devices) |
| `255` | `Unknown` | Type UEFI non reconnu |

```rust
#[repr(u32)]
pub enum MemoryKind {
    Usable = 1,
    KernelCode = 2,
    KernelData = 3,
    BootloaderReclaimable = 4,
    PageTables = 5,
    AcpiReclaimable = 6,
    AcpiNvs = 7,
    Reserved = 8,
    Framebuffer = 9,
    Mmio = 10,
    Unknown = 255,
}
```

---

## Validation côté kernel

Le kernel DOIT valider `BootInfo` avant tout usage :

```rust
// kernel/src/arch/x86_64/boot/early_init.rs
pub fn validate_boot_info(info: &BootInfo) -> Result<(), BootError> {
    if info.magic != BOOT_INFO_MAGIC {
        return Err(BootError::InvalidMagic(info.magic));
    }
    if info.version != BOOT_INFO_VERSION {
        return Err(BootError::VersionMismatch {
            expected: BOOT_INFO_VERSION,
            got: info.version,
        });
    }
    if info.memory_region_count as usize > MAX_MEMORY_REGIONS {
        return Err(BootError::TooManyRegions);
    }
    Ok(())
}
```

---

## Invariants de la structure

1. `BootInfo` est alignée page (4 KiB) — jamais déplacée après création
2. Le pointeur `*const BootInfo` passé en RDI est physiquement valide
3. Après ExitBootServices, la région qui contient `BootInfo` est `BootloaderReclaimable`
4. `entropy` est remplie avant la dernière allocation (stabilité entropique)
5. `boot_tsc` est la dernière valeur écrite, juste avant `jmp` kernel
