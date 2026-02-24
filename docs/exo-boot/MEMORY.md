# Gestion mémoire dans exo-boot

---

## Vue d'ensemble

exo-boot doit gérer la mémoire dans un environnement très contraint :
- Pas de `std`, pas d'allocateur général fiable après ExitBootServices
- Toutes les allocations utilisent `BootServices::allocate_pool` / `allocate_pages`
- Après ExitBootServices, aucune nouvelle allocation n'est possible
- La carte mémoire finale est transmise telle quelle au kernel via `BootInfo`

---

## Carte mémoire UEFI → Régions kernel

### Collecte (avant ExitBootServices)

```
bt.memory_map_size()              → taille hint
bt.allocate_pool(len + 8 KiB)    → buffer
bt.memory_map(buffer)             → MemoryMap<'_> + MemoryMapKey
```

Chaque descripteur UEFI a 40 octets :
```
PhysicalStart : u64
NumberOfPages : u64
Type          : EfiMemoryType (u32)
Attribute     : u64
```

### Conversion en MemoryKind

| EfiMemoryType (UEFI) | MemoryKind (exo-boot) |
|---------------------|----------------------|
| EfiLoaderCode (1) | BootloaderReclaimable |
| EfiLoaderData (2) | BootloaderReclaimable |
| EfiBootServicesCode (3) | BootloaderReclaimable |
| EfiBootServicesData (4) | BootloaderReclaimable |
| EfiRuntimeServicesCode (5) | Reserved |
| EfiRuntimeServicesData (6) | Reserved |
| EfiConventionalMemory (7) | Usable |
| EfiUnusableMemory (8) | Reserved |
| EfiACPIReclaimMemory (9) | AcpiReclaimable |
| EfiACPIMemoryNVS (10) | AcpiNvs |
| EfiMemoryMappedIO (11) | Mmio |
| EfiMemoryMappedIOPortSpace (12) | Mmio |
| EfiPersistentMemory (14) | Usable |
| Autres | Unknown |

### Post-traitement

Après conversion, les régions chevauchantes sont annotées :
1. Région du kernel ELF chargé → `KernelCode` / `KernelData`
2. Tables de pages créées → `PageTables`
3. Région framebuffer GOP → `Framebuffer`
4. Fusionner les régions contiguës de même type

---

## Types de mémoire dans BootInfo

```
Adresse physique 0
  ┌─────────────────────────┐ 0x0000_0000
  │  Reserved / BIOS        │  (Real Mode IVT, BIOS Data Area)
  ├─────────────────────────┤ 0x0000_1000
  │  Usable                 │  (RAM basse libre)
  ├─────────────────────────┤ ~0x0009_F000
  │  Reserved               │  (EBDA, ROM, VGA)
  ├─────────────────────────┤ 0x000F_0000
  │  Reserved               │  (Legacy BIOS ROM 640K+)
  ├─────────────────────────┤ 0x0010_0000  (1 MiB)
  │  BootloaderReclaimable  │  (exo-boot.efi, buffers boot)
  ├─────────────────────────┤ ~kaslr_base
  │  KernelCode             │  PT_LOAD exécutable
  │  KernelData             │  PT_LOAD données
  ├─────────────────────────┤ kaslr_base + kernel_size
  │  PageTables             │  PML4 + PDPT + PD (16 pages)
  ├─────────────────────────┤
  │  Usable                 │  RAM libre pour kernel heap
  ├─────────────────────────┤ ~fb_phys_addr
  │  Framebuffer            │  GOP linéaire framebuffer
  ├─────────────────────────┤
  │  AcpiReclaimable        │  Tables ACPI (RSDT/XSDT...)
  │  AcpiNvs                │  NVS persistant
  │  Reserved               │  Firmware, MMIO
  └─────────────────────────┘ Top RAM
```

---

## Tables de pages initiales

### Structure PML4 construite par exo-boot

```
PML4 (niveau 4, 512 entrées × 8 octets = 4 KiB)
├── [0]   → PDPT_low   (mapping identité 0 → 512 GiB)
│   ├── [0] → PD_0  → 512 × 2 MiB pages (0 GiB → 1 GiB)
│   ├── [1] → PD_1  → 512 × 2 MiB pages (1 GiB → 2 GiB)
│   ├── [2] → PD_2  → 512 × 2 MiB pages (2 GiB → 3 GiB)
│   └── [3] → PD_3  → 512 × 2 MiB pages (3 GiB → 4 GiB)
│
└── [511] → PDPT_high  (higher-half kernel)
    └── [510] → PD_kernel → kernel pages (PRESENT|WRITABLE|HUGE)
    └── [511] → ...
```

### Flags des grandes pages (2 MiB)

| Bit | Nom | Valeur | Description |
|-----|-----|--------|-------------|
| 0 | PRESENT | 0x001 | Page présente en mémoire |
| 1 | WRITABLE | 0x002 | Page accessible en écriture |
| 2 | USER | 0x004 | Accessible en mode utilisateur (ring 3) |
| 3 | WRITE_THROUGH | 0x008 | Cache write-through |
| 4 | NO_CACHE | 0x010 | Désactive le cache |
| 5 | ACCESSED | 0x020 | Bit accès (mis par CPU) |
| 6 | DIRTY | 0x040 | Bit modification (mis par CPU) |
| 7 | HUGE | 0x080 | Grande page 2 MiB (au niveau PD) |
| 8 | GLOBAL | 0x100 | Page globale (non invalidée par CR3 flush) |
| 63 | NO_EXECUTE | 1<<63 | Exécution interdite (NXE activé dans EFER) |

### Mappings créés

| Zone virtuelle | Zone physique | Taille | Flags |
|---------------|--------------|--------|-------|
| 0x0000_0000 → 0xFFFF_FFFF | idem (identité) | 4 GiB | P, W |
| 0xFFFF_FFFF_8000_0000 + offset | kaslr_base | kernel_size | P, W (+ NX pour .data) |
| BootInfo physique | idem | 1 page | P, W |

---

## Détection ACPI RSDP

### Méthode UEFI (priorité 1)

```rust
// Itérer les EFI Configuration Tables
for entry in system_table.config_table() {
    match entry.guid {
        // ACPI 2.0 (préféré)
        ACPI_2_GUID => return entry.address as u64,
        // ACPI 1.0 (fallback)
        ACPI_1_GUID => rsdp1 = entry.address as u64,
    }
}
return rsdp2.unwrap_or(rsdp1);
```

### Méthode BIOS (fallback / chemin BIOS)

```
Scan de 0x000E_0000 à 0x000F_FFFF (128 KiB, BIOS extended ROM)
Alignement : tous les 16 octets
Signature  : b"RSD PTR " (8 octets)
Checksum   : somme des 20 premiers octets == 0 (mod 256)
ACPI 2.0   : vérifier extended_checksum sur 36 octets
```

---

## Heap bootloader (UEFI)

Pendant la phase de boot, exo-boot utilise l'allocateur UEFI pool :

```rust
// Fourni par uefi-services via uefi::allocator::Allocator
// Implémente GlobalAlloc → Box, Vec disponibles en phase UEFI

// IMPORTANT : Toutes les allocations sont invalides après ExitBootServices
// La mémoire LOADER_DATA est marquée BootloaderReclaimable
// Le kernel peut la récupérer après son propre allocateur initialisé
```

---

## Contraintes mémoire importantes

| Règle | Description |
|-------|-------------|
| BOOT-03 | `BootInfo` alloué dans une région qui survit à ExitBootServices |
| BOOT-06 | Aucune allocation après ExitBootServices |
| MAP-01 | `MAX_MEMORY_REGIONS = 256` : carte tronquée si dépassé |
| MAP-02 | Régions triées par `base` croissant dans `BootInfo` |
| MAP-03 | Aucune région de taille 0 dans la carte finale |
| PG-01 | PML4 alloué avec `allocate_pages` (aligné page) pas `allocate_pool` |
