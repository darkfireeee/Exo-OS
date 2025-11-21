# 📋 TODO List - Exo-OS Kernel

**Dernière mise à jour:** 21 novembre 2025

## 🔴 Priorité Critique

### 1. Créer un Point d'Entrée Exécutable
**Statut:** ⏳ EN ATTENTE  
**Difficulté:** 🔥🔥🔥

**Tâches:**
- [ ] Créer `boot/boot_stub.c` avec `_start` multiboot2
- [ ] Configurer stack initiale (4KB minimum)
- [ ] Parser multiboot2 info structure
- [ ] Initialiser mémoire physique early-stage
- [ ] Appeler `rust_kernel_main()` depuis `libexo_kernel.a`
- [ ] Configurer linker script (`linker.ld`)
  - [ ] Section `.text` à 0x100000 (1MB)
  - [ ] Section `.rodata` après `.text`
  - [ ] Section `.data` + `.bss` alignées 4KB
  - [ ] Stack à 0x80000 (512KB)

**Fichiers à créer:**
```
kernel/
├── boot/
│   ├── boot_stub.c       # Point d'entrée C
│   └── early_setup.asm   # Setup GDT/IDT initial
├── linker.ld             # Script de linkage
└── Makefile              # Build system final
```

**Commande de build:**
```bash
# Compiler boot stub
gcc -m64 -ffreestanding -nostdlib -c boot/boot_stub.c -o boot/boot_stub.o

# Compiler early setup
nasm -f elf64 boot/early_setup.asm -o boot/early_setup.o

# Linker final
ld -n -T linker.ld -o exo_kernel.elf \
   boot/boot_stub.o \
   boot/early_setup.o \
   target/x86_64-unknown-none/release/libexo_kernel.a
```

---

### 2. Réactiver Support SMP (Multiprocessing)
**Statut:** ⚠️ DÉSACTIVÉ  
**Difficulté:** 🔥🔥

**Problème actuel:** `trampoline.asm` utilise directives NASM incompatibles avec `global_asm!()`

**Solution:**
- [ ] Modifier `build.rs` pour compiler `trampoline.asm` avec NASM
  ```rust
  cc::Build::new()
      .file("src/arch/x86_64/boot/trampoline.asm")
      .compiler("nasm")
      .flag("-f").flag("elf64")
      .flag("-o").flag("trampoline.o")
      .compile("trampoline");
  ```
- [ ] Déclarer symboles externes dans `smp.rs`
  ```rust
  extern "C" {
      fn trampoline_start();
      fn trampoline_end();
  }
  ```
- [ ] Décommenter code SMP initialization
- [ ] Tester sur QEMU avec `-smp 4`

**Fichier:** `kernel/src/arch/x86_64/cpu/smp.rs` ligne 21

---

### 3. Tester Boot sur QEMU
**Statut:** ⏳ EN ATTENTE (dépend de #1)  
**Difficulté:** 🔥🔥

**Prérequis:**
- Point d'entrée exécutable compilé
- Image ISO avec GRUB multiboot2

**Étapes:**
- [ ] Créer configuration GRUB (`grub.cfg`)
  ```
  menuentry "Exo-OS" {
      multiboot2 /boot/exo_kernel.elf
      boot
  }
  ```
- [ ] Générer ISO bootable
  ```bash
  grub-mkrescue -o exo_os.iso iso/
  ```
- [ ] Lancer QEMU
  ```bash
  qemu-system-x86_64 \
      -cdrom exo_os.iso \
      -m 256M \
      -serial stdio \
      -no-reboot \
      -no-shutdown
  ```
- [ ] Vérifier sortie série (premiers logs kernel)

**Tests à valider:**
- [ ] Boot réussi (pas de triple fault)
- [ ] GDT/IDT chargés correctement
- [ ] Mémoire détectée (multiboot memory map)
- [ ] Allocateur heap fonctionnel
- [ ] Premier log `"Exo-OS kernel initialized"`

---

## 🟡 Priorité Haute

### 4. Finaliser Détection Topologie CPU
**Statut:** 📝 TODO  
**Difficulté:** 🔥🔥  
**Fichier:** `kernel/src/arch/x86_64/cpu/topology.rs`

**Implémentation requise:**
```rust
pub fn get_intel_topology_level(level: u32) -> Option<TopologyLevel> {
    unsafe {
        // CPUID leaf 0xB (Extended Topology)
        let result = core::arch::x86_64::__cpuid_count(0xB, level);
        
        if result.eax == 0 && result.ebx == 0 {
            return None; // Invalid level
        }
        
        Some(TopologyLevel {
            level_type: (result.ecx >> 8) & 0xFF,  // Bits 8-15
            level_shift: result.eax & 0x1F,        // Bits 0-4
            processor_count: result.ebx & 0xFFFF,  // Bits 0-15
        })
    }
}
```

**Tests:**
- [ ] CPU Intel (Xeon, Core i7)
- [ ] CPU AMD (Ryzen, EPYC) - leaf 0x8000001E
- [ ] Single-core vs Multi-core
- [ ] SMT (Hyper-Threading) detection

---

### 5. Cleanup Warnings (231 → <50)
**Statut:** 📝 TODO  
**Difficulté:** 🔥

**Catégories:**

#### A. Variables inutilisées (~180 warnings)
```bash
cargo fix --lib -p exo-kernel --allow-dirty
```
Ensuite, revue manuelle pour:
- [ ] Ajouter `#[allow(dead_code)]` sur code préparatoire
- [ ] Préfixer `_` variables debug (`_buffer`, `_width`)
- [ ] Supprimer imports réellement inutiles

#### B. Static mut refs (~15 warnings)
Migrer vers Rust 2024 safe pattern:
```rust
// AVANT
static mut GLOBAL: Manager = Manager::new();
unsafe { &mut GLOBAL }

// APRÈS
use core::cell::SyncUnsafeCell;
static GLOBAL: SyncUnsafeCell<Manager> = SyncUnsafeCell::new(Manager::new());
unsafe { &mut *GLOBAL.get() }
```

**Fichiers à migrer:**
- [ ] `kernel/src/memory/physical/mod.rs`
- [ ] `kernel/src/memory/physical/numa.rs`
- [ ] `kernel/src/memory/heap/cpu_slab.rs`
- [ ] `kernel/src/arch/x86_64/gdt.rs`

#### C. Naming conventions (~6 warnings)
```rust
// AVANT
pub static cascade_interrupt: extern "C" fn() = ...;

// APRÈS
pub static CASCADE_INTERRUPT: extern "C" fn() = ...;
```

**Fichier:** `kernel/src/arch/x86_64/interrupts/handlers.rs` lignes 376-389

---

### 6. Implémenter Allocateur Heap dans lib.rs
**Statut:** 📝 TODO  
**Difficulté:** 🔥  
**Fichier:** `kernel/src/lib.rs`

**Problème:** Actuellement aucun `#[global_allocator]` dans la bibliothèque

**Solution:**
```rust
use exo_kernel::memory::heap::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Allocation failed: {:?}", layout);
}
```

**Note:** À initialiser dans boot stub avec `init_heap()`

---

## 🟢 Priorité Moyenne

### 7. Documentation API
**Statut:** 📝 TODO  
**Difficulté:** 🔥

- [ ] Générer rustdoc
  ```bash
  cargo doc --no-deps --lib -p exo-kernel
  ```
- [ ] Ajouter exemples dans docstrings
- [ ] Documenter unsafe functions
- [ ] Créer guide d'architecture (`Docs/ARCHITECTURE.md`)

---

### 8. Tests Unitaires
**Statut:** 📝 TODO  
**Difficulté:** 🔥🔥

**Framework:** `custom_test_frameworks` (no_std)

```rust
#![cfg_attr(test, feature(custom_test_frameworks))]
#![cfg_attr(test, test_runner(crate::test_runner))]

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    for test in tests {
        test();
    }
}

#[test_case]
fn test_cpuid() {
    let (eax, _, _, _) = unsafe { cpuid::cpuid(0x0) };
    assert!(eax > 0, "CPUID leaf 0 should return max leaf");
}
```

**Modules à tester:**
- [ ] `memory::physical` (allocation/deallocation)
- [ ] `memory::virtual` (page mapping)
- [ ] `arch::cpu::cpuid` (feature detection)
- [ ] `arch::interrupts` (IDT setup)

---

### 9. CI/CD Pipeline
**Statut:** 📝 TODO  
**Difficulté:** 🔥

**GitHub Actions:**
```yaml
name: Build and Test

on: [push, pull_request]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: nightly
          override: true
      - run: cargo build --release --lib
      - run: cargo test --lib
```

**À ajouter:**
- [ ] Vérification formatage (`cargo fmt --check`)
- [ ] Linting (`cargo clippy -- -D warnings`)
- [ ] Build ISO + test QEMU headless
- [ ] Badge statut dans README

---

## 🔵 Priorité Basse

### 10. Optimisations
**Statut:** 📝 TODO  
**Difficulté:** 🔥

- [ ] Profiling avec `perf` dans QEMU
- [ ] Analyse taille binaire (`cargo bloat`)
- [ ] LTO expérimental (`lto = "fat"` déjà activé)
- [ ] PGO (Profile-Guided Optimization)

---

### 11. Support ARM64 (aarch64)
**Statut:** 🔮 FUTUR  
**Difficulté:** 🔥🔥🔥

- [ ] Abstraire architecture dans `arch/mod.rs`
- [ ] Implémenter `arch/aarch64/`
- [ ] Bootloader U-Boot/UEFI
- [ ] Test sur Raspberry Pi 4

---

### 12. Network Stack (Modules Désactivés)
**Statut:** 🔮 FUTUR  
**Fichiers:** `userland/net_service`, `kernel/src/net/`

- [ ] Réactiver modules réseau
- [ ] Implémenter TCP/IP stack
- [ ] Drivers virtio-net, e1000
- [ ] Socket API

---

### 13. Filesystem (Modules Désactivés)
**Statut:** 🔮 FUTUR  
**Fichiers:** `userland/fs_service`, `kernel/src/fs/`

- [ ] Réactiver modules VFS
- [ ] Support ext4, FAT32
- [ ] Drivers AHCI, NVMe
- [ ] Montage initramfs

---

## 📊 Progression Globale

| Milestone | Statut | Progression |
|-----------|--------|-------------|
| **Compilation kernel** | ✅ Terminé | 100% |
| **Boot stub + linker** | ⏳ En cours | 0% |
| **Boot QEMU** | 📝 TODO | 0% |
| **SMP support** | ⚠️ Désactivé | 30% |
| **Tests unitaires** | 📝 TODO | 0% |
| **Documentation** | 📝 TODO | 20% |
| **Modules userland** | 🔮 Futur | 0% |

**Progression totale:** ~25% 🟡

---

## 🎯 Objectifs Court Terme (1-2 semaines)

1. ✅ ~~Compiler bibliothèque kernel sans erreurs~~
2. ⏳ Créer boot stub + linker script
3. ⏳ Générer image ISO bootable
4. ⏳ Premier boot QEMU réussi

**Prochain rapport:** Après boot QEMU fonctionnel

---

**Légende:**
- ✅ Terminé
- ⏳ En cours
- 📝 TODO
- ⚠️ Bloqué/Workaround
- 🔮 Futur lointain
- 🔥 Difficulté (🔥=facile, 🔥🔥🔥=difficile)
