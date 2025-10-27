# Scripts Exo-OS

Ce dossier contient tous les scripts de build et d'exécution pour Exo-OS.

## Scripts Linux/WSL (Bash)

### `build-iso.sh`
Script principal de build qui :
1. Compile le bootloader (NASM)
2. Compile le kernel Rust
3. Lie le bootloader et le kernel
4. Crée une image ISO bootable avec GRUB

**Usage:**
```bash
cd /mnt/c/Users/Eric/Documents/Exo-OS
./scripts/build-iso.sh
```

**Prérequis:**
- NASM (assembleur)
- Rust nightly avec rust-src
- GRUB tools (grub-mkrescue, grub-pc-bin)
- xorriso, mtools

**Installation des prérequis (Ubuntu/WSL):**
```bash
sudo apt-get update
sudo apt-get install -y nasm grub-pc-bin grub-common xorriso mtools qemu-system-x86
rustup install nightly
rustup component add rust-src --toolchain nightly
```

### `run-qemu.sh`
Lance QEMU avec l'image ISO créée.

**Usage:**
```bash
./scripts/run-qemu.sh
```

**Options QEMU additionnelles:**
```bash
./scripts/run-qemu.sh -enable-kvm  # Avec accélération KVM
./scripts/run-qemu.sh -d int       # Avec debug des interruptions
```

**Contrôles QEMU:**
- `Ctrl+A` puis `X` : Quitter QEMU
- `Ctrl+A` puis `C` : Console monitor QEMU

### `clean.sh`
Nettoie tous les fichiers de build.

**Usage:**
```bash
./scripts/clean.sh
```

## Scripts Windows (PowerShell)

### `build.ps1`
Version Windows du script de build (utilise cargo bootimage - déprécié).

**Note:** Ce script est conservé pour référence mais n'est plus utilisé avec le nouveau système GRUB.

### `run-qemu.ps1`
Version Windows du script QEMU.

**Usage:**
```powershell
.\scripts\run-qemu.ps1
```

## Workflow de développement

### Build complet et test
```bash
# Depuis WSL
cd /mnt/c/Users/Eric/Documents/Exo-OS

# Nettoyer
./scripts/clean.sh

# Builder
./scripts/build-iso.sh

# Tester
./scripts/run-qemu.sh
```

### Build rapide (sans nettoyage)
```bash
./scripts/build-iso.sh && ./scripts/run-qemu.sh
```

## Structure des builds

```
build/
├── multiboot2_header.o    # Header multiboot2 compilé
├── boot.o                 # Bootloader assembleur compilé
├── kernel.bin             # Binaire final (bootloader + kernel)
├── exo-os.iso             # Image ISO bootable
└── isofiles/              # Structure de l'ISO
    └── boot/
        ├── kernel.bin     # Kernel copié
        └── grub/
            └── grub.cfg   # Configuration GRUB
```

## Dépannage

### "NASM not found"
```bash
sudo apt-get install nasm
```

### "grub-mkrescue not found"
```bash
sudo apt-get install grub-pc-bin grub-common xorriso mtools
```

### "Rust nightly not found"
```bash
rustup install nightly
rustup component add rust-src --toolchain nightly
```

### "kernel.bin is not a valid multiboot2 binary"
Vérifiez que le header multiboot2 est correctement aligné et que le linker script est à jour.

### QEMU ne démarre pas
- Vérifiez que l'ISO existe : `ls -lh build/exo-os.iso`
- Vérifiez les logs de GRUB
- Testez avec debug : `./scripts/run-qemu.sh -d int,cpu_reset`

## Permissions

N'oubliez pas de rendre les scripts exécutables :
```bash
chmod +x scripts/*.sh
```
