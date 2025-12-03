# ELF Loader Documentation

## Overview

Le module `loader` fournit les fonctionnalités pour charger des exécutables ELF64 en mémoire et préparer leur exécution.

## Modules

### `elf64.rs` - Structures ELF64

Définitions des structures du format ELF64 selon la spécification System V ABI pour AMD64.

#### Structures principales

```rust
// Header ELF64 (64 bytes)
struct Elf64Header {
    e_ident: [u8; 16],     // Magic + identification
    e_type: u16,           // Type (EXEC=2, DYN=3)
    e_machine: u16,        // Architecture (x86_64=62)
    e_entry: u64,          // Point d'entrée
    e_phoff: u64,          // Offset program headers
    e_shoff: u64,          // Offset section headers
    // ...
}

// Program Header (56 bytes)
struct Elf64ProgramHeader {
    p_type: u32,           // PT_LOAD, PT_INTERP, etc.
    p_flags: u32,          // PF_R, PF_W, PF_X
    p_offset: u64,         // Offset dans le fichier
    p_vaddr: u64,          // Adresse virtuelle
    p_filesz: u64,         // Taille dans le fichier
    p_memsz: u64,          // Taille en mémoire
    // ...
}
```

#### Types de segment (p_type)

| Constante | Valeur | Description |
|-----------|--------|-------------|
| PT_NULL | 0 | Entrée ignorée |
| PT_LOAD | 1 | Segment à charger |
| PT_DYNAMIC | 2 | Informations dynamiques |
| PT_INTERP | 3 | Chemin du linker |
| PT_TLS | 7 | Thread-Local Storage |
| PT_GNU_STACK | 0x6474e551 | Permissions stack |

#### Flags de permission (p_flags)

| Flag | Valeur | Description |
|------|--------|-------------|
| PF_X | 0x1 | Exécutable |
| PF_W | 0x2 | Writable |
| PF_R | 0x4 | Readable |

### `process_image.rs` - Image Processus

Représente un exécutable chargé en mémoire.

```rust
struct LoadedElf {
    entry_point: VirtualAddress,    // Point d'entrée
    load_bias: VirtualAddress,      // Bias pour PIE
    segments: Vec<LoadedSegment>,   // Segments chargés
    tls_template: Option<TlsTemplate>, // TLS si présent
    interpreter: Option<String>,    // Linker dynamique
    phdr_addr: VirtualAddress,      // Pour AT_PHDR
}

struct LoadedSegment {
    vaddr: VirtualAddress,  // Adresse (alignée page)
    mem_size: usize,        // Taille mémoire
    file_size: usize,       // Taille fichier
    flags: SegmentFlags,    // R/W/X
}
```

### `mod.rs` - API principale

```rust
/// Charger un ELF depuis un buffer
pub fn load_elf(
    elf_data: &[u8],
    base_address: Option<VirtualAddress>
) -> ElfResult<LoadedElf>;
```

## Auxiliary Vector (auxv)

Le vecteur auxiliaire est passé au programme au démarrage :

| Type | Valeur | Description |
|------|--------|-------------|
| AT_PHDR | 3 | Adresse des program headers |
| AT_PHENT | 4 | Taille d'un program header |
| AT_PHNUM | 5 | Nombre de program headers |
| AT_PAGESZ | 6 | Taille de page (4096) |
| AT_ENTRY | 9 | Point d'entrée |
| AT_BASE | 7 | Base du linker dynamique |
| AT_RANDOM | 25 | 16 bytes aléatoires |

## Usage

```rust
use crate::loader::{load_elf, LoadedElf};
use crate::memory::VirtualAddress;

// Charger un ELF
let elf_data = fs::read("/bin/hello")?;
let loaded = load_elf(&elf_data, None)?;

// Préparer le contexte utilisateur
let ctx = UserContext::new(
    loaded.entry_point,
    stack_pointer,
);

// Arguments
ctx.set_args(argc, argv_ptr, envp_ptr);

// Transition vers user mode
unsafe { jump_to_usermode(&ctx); }
```

## Support

### Supporté
- ✅ ELF64 (ELFCLASS64)
- ✅ Little Endian (ELFDATA2LSB)
- ✅ x86_64 (EM_X86_64)
- ✅ Exécutables (ET_EXEC)
- ✅ PIE (ET_DYN)
- ✅ PT_LOAD, PT_TLS, PT_INTERP
- ✅ Auxiliary vector

### Non supporté (futur)
- ⏳ Relocations dynamiques
- ⏳ Chargement de bibliothèques partagées
- ⏳ ld-linux.so.2 intégré
