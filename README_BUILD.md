# Exo-OS - Système d'exploitation personnalisé

Exo-OS est un système d'exploitation microkernel expérimental écrit en Rust avec un bootloader personnalisé utilisant Multiboot2 et GRUB.

## 🚀 Quick Start

### Prérequis

**Sur Windows avec WSL (Ubuntu):**

```bash
# Ouvrir WSL Ubuntu
wsl

# Installer les dépendances
sudo apt-get update
sudo apt-get install -y \
    nasm \
    grub-pc-bin \
    grub-common \
    xorriso \
    mtools \
    qemu-system-x86 \
    build-essential

# Installer Rust nightly
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup install nightly
rustup component add rust-src --toolchain nightly
```

### Build et Exécution

```bash
# Aller dans le répertoire du projet (dans WSL)
cd /mnt/c/Users/Eric/Documents/Exo-OS

# Rendre les scripts exécutables
chmod +x scripts/*.sh

# Nettoyer les anciens builds (optionnel)
./scripts/clean.sh

# Compiler et créer l'ISO
./scripts/build-iso.sh

# Lancer dans QEMU
./scripts/run-qemu.sh
```

## 📁 Structure du Projet

```
Exo-OS/
├── bootloader/              # Bootloader personnalisé Multiboot2
│   ├── multiboot2_header.asm  # Header Multiboot2
│   ├── boot.asm               # Code de boot (32->64 bit)
│   └── grub.cfg               # Configuration GRUB
│
├── kernel/                  # Noyau Rust
│   └── src/
│       ├── lib.rs           # Point d'entrée principal
│       ├── main.rs          # Stub (vide pour multiboot2)
│       ├── arch/            # Code spécifique à l'architecture
│       ├── drivers/         # Pilotes matériels
│       ├── memory/          # Gestion de la mémoire
│       ├── scheduler/       # Ordonnanceur
│       ├── ipc/             # Communication inter-processus
│       └── syscall/         # Appels système
│
├── scripts/                 # Scripts de build et utilitaires
│   ├── build-iso.sh         # Build complet + création ISO
│   ├── run-qemu.sh          # Lancement QEMU
│   ├── clean.sh             # Nettoyage
│   └── README.md            # Documentation des scripts
│
├── build/                   # Répertoire de build (généré)
│   ├── kernel.bin           # Binaire final
│   ├── exo-os.iso           # Image ISO bootable
│   └── isofiles/            # Structure ISO
│
├── Docs/                    # Documentation technique
├── bootloader-linker.ld     # Script de linkage pour le bootloader
├── linker.ld                # Script de linkage du kernel
├── x86_64-unknown-none.json # Target Rust personnalisé
└── Cargo.toml               # Configuration Rust workspace

```

## 🛠️ Architecture Technique

### Bootloader Multiboot2

Le bootloader personnalisé :
1. **multiboot2_header.asm** : Définit le header Multiboot2 reconnu par GRUB
2. **boot.asm** : 
   - Vérifie le magic number Multiboot2
   - Active le mode long (64-bit)
   - Configure les page tables
   - Transfère le contrôle au kernel Rust

### Kernel Rust

Le kernel est un microkernel avec :
- **Architecture x86_64** : GDT, IDT, interruptions
- **Gestion mémoire** : Frame allocator, page tables, heap allocator
- **Ordonnanceur** : Multi-threading avec context switching
- **IPC** : Channels et messages pour la communication
- **Syscalls** : Interface pour les processus utilisateur
- **Drivers** : Serial UART 16550, futures: VGA, PCI, etc.

### Processus de Build

```
┌─────────────────────┐
│  multiboot2_header  │
│     (.asm)          │
└──────────┬──────────┘
           │ NASM
           ▼
    ┌──────────────┐      ┌─────────────┐
    │  boot.asm    │      │  Kernel     │
    │              │      │  (Rust)     │
    └──────┬───────┘      └──────┬──────┘
           │ NASM               │ rustc
           ▼                    ▼
    ┌──────────────┐      ┌─────────────┐
    │  boot.o      │      │ libexo_     │
    │              │      │ kernel.a    │
    └──────┬───────┘      └──────┬──────┘
           │                     │
           └──────────┬──────────┘
                      │ ld
                      ▼
              ┌───────────────┐
              │  kernel.bin   │
              │  (Multiboot2) │
              └───────┬───────┘
                      │ grub-mkrescue
                      ▼
              ┌───────────────┐
              │  exo-os.iso   │
              │  (Bootable)   │
              └───────────────┘
```

## 🔧 Développement

### Compiler uniquement le kernel

```bash
cd kernel
cargo +nightly build --target ../x86_64-unknown-none.json --release
```

### Compiler uniquement le bootloader

```bash
cd bootloader
nasm -f elf64 multiboot2_header.asm -o multiboot2_header.o
nasm -f elf64 boot.asm -o boot.o
```

### Debug avec QEMU

```bash
# Avec interruptions et resets
./scripts/run-qemu.sh -d int,cpu_reset

# Avec monitor QEMU
./scripts/run-qemu.sh -monitor stdio

# Avec KVM (si disponible sous Linux)
./scripts/run-qemu.sh -enable-kvm
```

### Vérifier le binaire Multiboot2

```bash
grub-file --is-x86-multiboot2 build/kernel.bin
echo $?  # 0 = valide, 1 = invalide
```

## 📊 État du Projet

### ✅ Fonctionnel
- Bootloader Multiboot2 complet
- Transition 32-bit → 64-bit
- Kernel Rust avec allocation mémoire
- Driver serial UART
- GDT, IDT, gestion des interruptions
- Structures IPC de base
- Ordonnanceur (structure)

### 🚧 En Développement
- Gestion complète de la mémoire
- Ordonnanceur fonctionnel
- Système de fichiers
- Support multi-core
- Drivers PCI, VGA
- Espace utilisateur

### 📋 TODO
- [ ] Implémenter l'allocateur de frames
- [ ] Tester l'ordonnanceur multi-thread
- [ ] Ajouter un système de fichiers
- [ ] Support des processus utilisateur
- [ ] Shell interactif
- [ ] Tests unitaires et d'intégration

## 🐛 Dépannage

### "No bootable medium found"
L'ISO n'est pas correctement créée. Vérifiez :
```bash
ls -lh build/exo-os.iso
grub-file --is-x86-multiboot2 build/kernel.bin
```

### "Invalid multiboot2 magic"
Le header Multiboot2 n'est pas au bon endroit. Vérifiez le linker script.

### "Kernel panic"
Consultez la sortie série (QEMU `-serial stdio`).

### Erreurs de compilation Rust
```bash
rustup update nightly
rustup component add rust-src --toolchain nightly
```

## 📖 Documentation

- `Docs/readme_kernel.txt` - Architecture du kernel
- `Docs/readme_memory_and_scheduler.md` - Mémoire et ordonnanceur
- `Docs/readme_syscall_et_drivers.md` - Syscalls et drivers
- `Docs/readme_x86_64_et_c_compact.md` - x86_64 et compatibilité C
- `scripts/README.md` - Documentation des scripts

## 📝 License

Voir le fichier LICENSE pour plus de détails.

## 🙏 Remerciements

- Rust embedded community
- OSDev wiki
- Multiboot2 specification
- GRUB project

---

**Note:** Ce projet est expérimental et à but éducatif. Ne l'utilisez pas en production!
