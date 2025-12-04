# 🚀 Exo-OS v0.5.0 "Quantum Leap"

**Système d'exploitation moderne écrit en Rust avec boot C/ASM et shell interactif complet**

[![License](https://img.shields.io/badge/GPL-2.0license-blue.svg)](LICENSE)
[![Build](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![Version](https://img.shields.io/badge/version-0.5.0-orange.svg)]()

---

## ✨ Nouveautés v0.5.0

- 🔗 **Linkage C/Rust complet** - Boot multiboot2 en ASM → C → Rust
- 🐚 **Exo-Shell** - Shell interactif avec 14 commandes VFS
- 🧠 **Heap allocator stable** - Bugs critiques corrigés
- 📦 **Build automatisé** - Script en 8 étapes  
- 🚀 **Boot complet validé** - Testé en QEMU, prêt pour production

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

### v0.6.0 (Prochain)
- ⏳ Driver clavier PS/2
- ⏳ Entrée shell interactive
- ⏳ Montage VFS / FAT32

### v0.7.0 (Futur)
- 📅 Processus userland
- 📅 Syscalls (fork, exec, read, write)
- 📅 ELF loader

### v1.0.0 (Vision)
- 🎯 Network stack TCP/IP
- 🎯 Filesystem ext2
- 🎯 Multi-utilisateurs

Voir [roadmap complet](docs/roadmap_v0.5.0.md)

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

- **Code** : ~60,000 lignes (Rust + C + ASM)
- **Fichiers Rust** : 409 modules
- **Kernel** : 22MB (avec debug)
- **ISO** : 27MB bootable
- **Boot time** : ~2s (QEMU)

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

**Exo-OS v0.5.0 "Quantum Leap"**

*Making the impossible possible* 🚀

[Docs](docs/INDEX_COMPLET.md) • [Release](docs/v0.5.0_RELEASE_NOTES.md) • [Roadmap](docs/roadmap_v0.5.0.md)

⭐ **Star ce projet si vous l'aimez !** ⭐

</div>
