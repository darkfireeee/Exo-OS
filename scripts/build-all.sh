#!/bin/bash
# Script de build complet pour Exo-OS depuis WSL

set -e  # Arrêter en cas d'erreur

# Couleurs pour les messages
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}╔════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║   Build complet d'Exo-OS avec GRUB    ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════╝${NC}"
echo ""

# Vérifier qu'on est bien dans le bon répertoire
if [ ! -d "kernel" ] || [ ! -d "bootloader" ]; then
    echo -e "${RED}❌ Erreur: Ce script doit être exécuté depuis la racine du projet Exo-OS${NC}"
    exit 1
fi

# Créer le dossier build s'il n'existe pas
mkdir -p build

# Sourcer l'environnement Rust
source ~/.cargo/env

# Étape 1: Compiler le kernel Rust
echo -e "${YELLOW}[1/5]${NC} Compilation du kernel Rust..."
cd kernel
cargo build --release --target x86_64-unknown-none \
    --features "fusion_rings,hybrid_allocator" \
    -Z build-std=core,alloc,compiler_builtins \
    -Z build-std-features=compiler-builtins-mem

if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Échec de la compilation du kernel${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Kernel compilé avec succès${NC}"
cd ..


# Étape 2: Assembler le bootloader et le code de contexte
echo -e "${YELLOW}[2/5]${NC} Assemblage du bootloader multiboot2 et du context switch..."
nasm -f elf64 bootloader/multiboot2_header.asm -o build/multiboot2_header.o
nasm -f elf64 bootloader/boot.asm -o build/boot.o
as --64 kernel/src/scheduler/context_switch.S -o build/context_switch.o

if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Échec de l'assemblage du bootloader${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Bootloader assemblé${NC}"

# Étape 3: Linker le kernel avec le bootloader
echo -e "${YELLOW}[3/5]${NC} Liaison du kernel..."
ld -n -T bootloader/linker.ld \
    -o build/kernel.bin \
    build/multiboot2_header.o \
    build/boot.o \
    build/context_switch.o \
    target/x86_64-unknown-none/release/libexo_kernel.a

if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Échec de la liaison${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Kernel lié avec succès${NC}"

# Vérifier que le kernel a bien le header multiboot2
echo -e "${YELLOW}[4/5]${NC} Vérification du header multiboot2..."
if grub-file --is-x86-multiboot2 build/kernel.bin; then
    echo -e "${GREEN}✓ Header multiboot2 valide${NC}"
else
    echo -e "${RED}❌ Header multiboot2 invalide!${NC}"
    exit 1
fi

# Étape 4: Créer l'image ISO
echo -e "${YELLOW}[5/5]${NC} Création de l'image ISO bootable..."
mkdir -p isodir/boot/grub
cp build/kernel.bin isodir/boot/
cp bootloader/grub.cfg isodir/boot/grub/

grub-mkrescue -o exo-os.iso isodir

if [ $? -ne 0 ]; then
    echo -e "${RED}❌ Échec de la création de l'ISO${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Image ISO créée: exo-os.iso${NC}"

# Afficher les informations
echo ""
echo -e "${BLUE}╔════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║         Build terminé avec succès!    ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════╝${NC}"
echo ""
echo -e "📦 Fichiers générés:"
echo -e "   - ${GREEN}build/kernel.bin${NC} ($(du -h build/kernel.bin | cut -f1))"
echo -e "   - ${GREEN}exo-os.iso${NC} ($(du -h exo-os.iso | cut -f1))"
echo ""
echo -e "🚀 Pour tester:"
echo -e "   ${YELLOW}./scripts/run-qemu.sh${NC}  (depuis WSL)"
echo -e "   ${YELLOW}.\\scripts\\run-qemu.ps1${NC}  (depuis Windows)"
echo ""
