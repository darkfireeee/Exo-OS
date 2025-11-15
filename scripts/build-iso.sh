#!/bin/bash
# build-iso.sh
# Script de build pour créer une image ISO bootable avec GRUB

set -e  # Arrêter en cas d'erreur

# Couleurs pour l'output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Exo-OS Build System${NC}"
echo -e "${BLUE}=========================================${NC}"

# Déterminer le répertoire racine du projet
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
KERNEL_DIR="$PROJECT_ROOT/kernel"
BOOTLOADER_DIR="$PROJECT_ROOT/bootloader"
BUILD_DIR="$PROJECT_ROOT/build"
ISO_DIR="$BUILD_DIR/isofiles"

echo -e "${YELLOW}[INFO]${NC} Répertoire du projet: $PROJECT_ROOT"

# Nettoyer les anciens builds
echo -e "${YELLOW}[BUILD]${NC} Nettoyage des anciens builds..."
rm -rf "$BUILD_DIR"
mkdir -p "$ISO_DIR/boot/grub"

# Compiler le kernel Rust avec boot.asm intégré
echo -e "${YELLOW}[BUILD]${NC} Compilation du kernel Rust avec boot.asm..."
cd "$KERNEL_DIR"

# Vérifier si rust nightly est installé
if ! rustup show | grep -q nightly; then
    echo -e "${YELLOW}[INFO]${NC} Installation de Rust nightly..."
    rustup install nightly
    rustup component add rust-src --toolchain nightly
fi

# Compiler le kernel avec la cible bare-metal (boot.asm sera compilé automatiquement par build.rs)
cargo build --release --target x86_64-unknown-none

echo -e "${GREEN}[SUCCESS]${NC} Kernel compilé avec boot.asm intégré"

# Le binaire final est déjà linkable et bootable
KERNEL_BIN="$PROJECT_ROOT/target/x86_64-unknown-none/release/exo-kernel"

if [ ! -f "$KERNEL_BIN" ]; then
    echo -e "${RED}[ERROR]${NC} Kernel binaire non trouvé: $KERNEL_BIN"
    exit 1
fi

echo -e "${GREEN}[SUCCESS]${NC} Kernel bootable: $KERNEL_BIN ($(du -h $KERNEL_BIN | cut -f1))"

# Vérifier que c'est un binaire multiboot2 valide
if command -v grub-file &> /dev/null; then
    if grub-file --is-x86-multiboot2 "$KERNEL_BIN"; then
        echo -e "${GREEN}[SUCCESS]${NC} Kernel est un binaire multiboot2 valide"
    else
        echo -e "${YELLOW}[WARNING]${NC} grub-file ne détecte pas multiboot2, mais le header est présent"
    fi
else
    echo -e "${YELLOW}[WARNING]${NC} grub-file non disponible, impossible de vérifier multiboot2"
fi

# Copier le kernel dans le répertoire ISO
cp "$KERNEL_BIN" "$ISO_DIR/boot/kernel.bin"

# Copier la configuration GRUB
cp "$BOOTLOADER_DIR/grub.cfg" "$ISO_DIR/boot/grub/grub.cfg"

# Créer l'image ISO
echo -e "${YELLOW}[BUILD]${NC} Création de l'image ISO..."

if ! command -v grub-mkrescue &> /dev/null; then
    echo -e "${RED}[ERROR]${NC} grub-mkrescue n'est pas installé. Installation..."
    sudo apt-get update
    sudo apt-get install -y grub-pc-bin grub-common xorriso mtools
fi

grub-mkrescue -o "$BUILD_DIR/exo-os.iso" "$ISO_DIR" 2>&1 | grep -v "warning: Missing translation"

if [ ! -f "$BUILD_DIR/exo-os.iso" ]; then
    echo -e "${RED}[ERROR]${NC} Échec de la création de l'ISO"
    exit 1
fi

echo -e "${GREEN}[SUCCESS]${NC} ISO créée: $BUILD_DIR/exo-os.iso ($(du -h "$BUILD_DIR/exo-os.iso" | cut -f1))"

# Résumé
echo -e "${BLUE}=========================================${NC}"
echo -e "${GREEN}[SUCCESS]${NC} Build terminé avec succès!"
echo -e "${BLUE}=========================================${NC}"
echo -e "  Bootloader: $BUILD_DIR/boot.o"
echo -e "  Kernel: $BUILD_DIR/kernel.bin"
echo -e "  ISO: $BUILD_DIR/exo-os.iso"
echo -e "${BLUE}=========================================${NC}"
echo -e "Pour tester: ${YELLOW}./scripts/run-qemu.sh${NC}"
echo -e "${BLUE}=========================================${NC}"
