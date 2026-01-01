# 🚀 Exo-OS v0.6.0 "Multicore Dawn"

**Système d'exploitation moderne écrit en Rust avec SMP - Phase 1 100% complète, Phase 2 30%**

[![License](https://img.shields.io/badge/GPL-2.0license-blue.svg)](LICENSE)
[![Build](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![Version](https://img.shields.io/badge/version-0.6.0-orange.svg)]()
[![Tests](https://img.shields.io/badge/tests-50/50_passing-success.svg)]()
[![CPUs](https://img.shields.io/badge/SMP-4_CPUs_online-blue.svg)]()

---

## 🎯 État Actuel

**Phase 1:** ✅ **100% complète** (50/50 tests passés)  
**Phase 2:** 🟢 **30% en cours** (SMP Foundation complète, Scheduler integration en cours)

| Composant | Tests | Status |
|-----------|-------|--------|
| **Phase 1a - VFS** | 20/20 | ✅ 100% |
| **Phase 1b - Processus** | 15/15 | ✅ 100% |
| **Phase 1c - Signaux** | 10/10 | ✅ 100% |
| **Phase 1d - CoW** | 5/5 | ✅ 100% |
| **Phase 2a - SMP Bootstrap** | 8/8 | ✅ 100% |
| **Phase 2b - SMP Scheduler** | 0/10 | 🟡 0% |

**Documentation Phase 1:** [PHASE_1_VALIDATION.md](docs/current/PHASE_1_VALIDATION.md)  
**Documentation Phase 2:** [PHASE_2_SMP_COMPLETE.md](docs/current/phase/PHASE_2_SMP_COMPLETE.md)

---

## ✨ Fonctionnalités Validées v0.5.0

### Gestion Mémoire
- ✅ **Allocateur bitmap** - 512MB, frames 4KB
- ✅ **Heap allocator** - 64MB stable
- ✅ **mmap/munmap** - Allocation virtuelle
- ✅ **mprotect** - Gestion permissions
- ⚠️ **CoW** - Conceptuel (page fault handler à implémenter)

### Système de Fichiers Virtuels
- ✅ **tmpfs** - 5/5 tests (create, write, read, offset, size)
- ✅ **devfs** - 5/5 tests (/dev/null, /dev/zero)
- ✅ **procfs** - 5/5 tests (cpuinfo, meminfo, status, uptime)
- ✅ **Registry** - 5/5 tests (device major/minor)

### Gestion Processus
- ✅ **fork/wait** - 5/5 tests (PID alloc, zombie cleanup)
- ✅ **clone** - Thread support (CLONE_THREAD)
- ✅ **futex** - Synchronisation (WAIT/WAKE/REQUEUE)
- ✅ **exit/wait4** - Exit status propagation

### Signaux POSIX
- ✅ **Syscalls** - rt_sigaction, sigprocmask, kill, tgkill
- ✅ **Handler registration** - SIG_DFL, SIG_IGN, custom
- ✅ **Signal delivery** - Pending sets, masking
- ✅ **Signal frame** - Context save/restore

### Scheduler & Timer
- ✅ **3-Queue scheduler** - Real-time, normal, idle
- ✅ **Context switch** - windowed_switch.S validé
- ✅ **Timer preemption** - PIT 100Hz
- ✅ **Benchmark** - ~2000 cycles/switch

### SMP Multi-core (Nouveau! 🎉)
- ✅ **4 CPUs online** - 1 BSP + 3 APs
- ✅ **ACPI/MADT parsing** - Détection automatique
- ✅ **APIC/IO-APIC** - Initialisation complète
- ✅ **AP Bootstrap** - Trampoline 16→32→64 bit
- ✅ **IPI messaging** - INIT/SIPI sequences
- ✅ **SSE/FPU/AVX** - Init sur tous les cores
- ✅ **Tests Bochs** - Validé 4 CPUs stable
- 🟡 **Per-CPU scheduler** - En cours d'implémentation

---

## 🚀 Quick Start

### Compilation

```bash
# Clone et build
git clone https://github.com/darkfireeee/Exo-OS.git
cd Exo-OS
./scripts/build_complete.sh
```

### Test QEMU

```bash
qemu-system-x86_64 -cdrom build/exo_os.iso -m 128M -nographic -serial mon:stdio
```

**Sortie attendue :**
```
[KERNEL] ✓ Multiboot2, Heap, Scheduler OK
[SHELL] Exo-Shell v0.5.0 launched ✓

╔═══════════════════════════════════════╗
║  🚀 Interactive Kernel Shell v0.5.0   ║
╚═══════════════════════════════════════╝

exo-os:~$ _
```

---

## 🎯 Fonctionnalités

- ✅ **Boot multiboot2** avec GRUB (ASM→C→Rust)
- ✅ **Mode 64-bit** avec paging identity 8GB
- ✅ **Heap allocator** 10MB stable
- ✅ **Scheduler** round-robin préemptif
- ✅ **Exo-Shell** 14 commandes (ls, cat, mkdir, etc.)
- ✅ **VFS** API filesystem unifiée
- ⏳ **Keyboard** PS/2 (v0.6.0)
- ⏳ **FAT32** Lecture ISO (v0.6.0)

---

## 🐚 Exo-Shell - Commandes

```bash
help            # Aide
ls [path]       # Liste répertoire
cat <file>      # Affiche fichier
mkdir <dir>     # Crée répertoire
touch <file>    # Crée fichier
write <f> <txt> # Écrit dans fichier
rm <file>       # Supprime fichier
pwd / cd        # Navigation
version / exit  # Système
```

---

## 🏗️ Architecture

### Boot Sequence
```
GRUB → boot.asm (32→64bit) → boot.c (FFI) → rust_main() → Exo-Shell
```

### Mémoire Layout
```
0x0000_0000 - 0x0010_0000 : BIOS, VGA
0x0010_0000 - 0x0050_0000 : Kernel (4MB)
0x0050_0000 - 0x0050_4000 : Bitmap (16KB)
0x0080_0000 - 0x0120_0000 : Heap (10MB)
```

---

## 📚 Documentation

- 📖 **[Index complet](docs/INDEX_COMPLET.md)** - Toute la documentation
- 🔨 **[Build Guide](docs/BUILD_AND_TEST_GUIDE.md)** - Compilation et tests
- 📋 **[Release Notes](docs/v0.5.0_RELEASE_NOTES.md)** - Nouveautés v0.5.0
- 🔗 **[Linkage Report](docs/LINKAGE_SUCCESS_REPORT.md)** - Détails C/Rust
- 🧠 **[Heap Fix](docs/HEAP_ALLOCATOR_FIX.md)** - Correction allocator
- 🏗️ **[Architecture](docs/ARCHITECTURE_v0.5.0.md)** - Vue d'ensemble

---

## ��️ Roadmap

### ✅ v0.6.0 "Multicore Dawn" (Actuel)
- ✅ SMP Foundation - 4 CPUs online
- ✅ ACPI/APIC/IPI complet
- ✅ AP Bootstrap fonctionnel
- ✅ Tests multi-core Bochs

### v0.7.0 "Parallel Universe" (Prochain - 2-3 semaines)
- 🟡 Per-CPU scheduler queues
- 🟡 Load balancing (work stealing)
- 🟡 Thread migration entre CPUs
- 🟡 TLB shootdown
- 🟡 Lock-free logging
- 🟡 SMP stress tests

### v0.8.0 (2 mois)
- 📌 Network stack TCP/IP
- 📌 Socket API BSD
- 📌 Drivers réseau (VirtIO, E1000)

### v1.0.0 "Linux Crusher" (6-9 mois)
- 🎯 Filesystem ext4
- 🎯 Drivers Linux GPL-2.0
- 🎯 Security (capabilities, TPM)
- 🎯 Performance > Linux

Voir [ROADMAP complet](docs/current/ROADMAP.md)

---

## 🔨 Build manuel

```bash
# Kernel
cargo build --release --manifest-path kernel/Cargo.toml

# Boot objects
nasm -f elf64 kernel/src/arch/x86_64/boot/boot.asm -o build/boot_objs/boot.o
gcc -m64 -ffreestanding -c kernel/src/arch/x86_64/boot/boot.c -o build/boot_objs/boot_c.o
ar rcs build/boot_objs/libboot_combined.a build/boot_objs/*.o

# Linkage
ld -n -T linker.ld -o build/kernel.elf \
   build/boot_objs/libboot_combined.a \
   target/x86_64-unknown-none/release/libexo_kernel.a

# ISO
strip build/kernel.elf -o build/kernel_stripped.elf
mkdir -p build/iso/boot/grub
cp build/kernel_stripped.elf build/iso/boot/kernel.elf
cp bootloader/grub.cfg build/iso/boot/grub/
grub-mkrescue -o build/exo_os.iso build/iso
```

---

## 🧪 Tests

```bash
# QEMU standard
qemu-system-x86_64 -cdrom build/exo_os.iso -m 128M -nographic -serial mon:stdio

# QEMU debug
qemu-system-x86_64 -cdrom build/exo_os.iso -m 128M -nographic \
  -serial mon:stdio -d int,cpu_reset -no-reboot

# Tests Rust
cd kernel && cargo test
```

---

## 🤝 Contributing

Les contributions sont bienvenues !

1. Fork le repository
2. Créer une branche (`git checkout -b feature/Amazing`)
3. Commit (`git commit -m 'Add feature'`)
4. Push (`git push origin feature/Amazing`)
5. Ouvrir une Pull Request

Voir [CONTRIBUTING.md](CONTRIBUTING.md) pour les guidelines.

---

## 📊 Statistiques

- **Code** : ~65,000 lignes (Rust + C + ASM)
- **Fichiers Rust** : 420+ modules
- **CPUs supportés** : 4 (SMP)
- **Kernel** : 23MB (avec debug)
- **ISO** : 28MB bootable
- **Boot time** : ~2s (QEMU), ~400ms (SMP init)
- **Phase 1** : 100% (50/50 tests)
- **Phase 2** : 30% (SMP bootstrap OK)

---

## 📄 License

Projet sous licence MIT. Voir [LICENSE](LICENSE).

---

## 🙏 Remerciements

- OSDev Community
- Rust Community  
- GRUB & QEMU Projects

---

<div align="center">

**Exo-OS v0.6.0 "Multicore Dawn"**

*4 CPUs Strong, Performance Beyond* 🚀

[Docs](docs/INDEX.md) • [Phase 1](docs/current/PHASE_1_VALIDATION.md) • [Phase 2 SMP](docs/current/phase/PHASE_2_SMP_COMPLETE.md) • [Roadmap](docs/current/ROADMAP.md)

⭐ **Star ce projet si vous l'aimez !** ⭐

</div>
