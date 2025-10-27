#!/bin/bash
# run-qemu.sh
# Script pour lancer QEMU avec l'image ISO Exo-OS

set -e

# Couleurs
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Déterminer le répertoire racine du projet
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_ROOT/build"
ISO_FILE="$BUILD_DIR/exo-os.iso"
if [ ! -f "$ISO_FILE" ]; then
    ISO_FILE="$PROJECT_ROOT/exo-os.iso"
fi

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Exo-OS QEMU Runner${NC}"
echo -e "${BLUE}=========================================${NC}"

# Vérifier que l'ISO existe
if [ ! -f "$ISO_FILE" ]; then
    echo -e "${RED}[ERROR]${NC} ISO non trouvée: $ISO_FILE"
    echo -e "${YELLOW}[INFO]${NC} Exécutez d'abord: ./scripts/build-iso.sh"
    exit 1
fi

# Vérifier que QEMU est installé
if ! command -v qemu-system-x86_64 &> /dev/null; then
    echo -e "${RED}[ERROR]${NC} QEMU n'est pas installé. Installation..."
    sudo apt-get update
    sudo apt-get install -y qemu-system-x86
fi

echo -e "${GREEN}[QEMU]${NC} Lancement de Exo-OS..."
echo -e "${YELLOW}[INFO]${NC} ISO: $ISO_FILE"
echo -e "${YELLOW}[INFO]${NC} Pour quitter: Ctrl+A puis X"
echo -e "${BLUE}=========================================${NC}"

# Lancer QEMU avec l'ISO
qemu-system-x86_64 \
    -cdrom "$ISO_FILE" \
    -boot d \
    -m 512M \
    -serial stdio \
    -no-reboot \
    -no-shutdown \
    "$@"

echo -e "${BLUE}=========================================${NC}"
echo -e "${GREEN}[QEMU]${NC} Session terminée"
echo -e "${BLUE}=========================================${NC}"
