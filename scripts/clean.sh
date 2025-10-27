#!/bin/bash
# clean.sh
# Script pour nettoyer tous les fichiers de build

set -e

# Couleurs
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Exo-OS Cleanup${NC}"
echo -e "${BLUE}=========================================${NC}"

# Nettoyer le répertoire build
if [ -d "$PROJECT_ROOT/build" ]; then
    echo -e "${YELLOW}[CLEAN]${NC} Suppression du répertoire build/..."
    rm -rf "$PROJECT_ROOT/build"
    echo -e "${GREEN}[SUCCESS]${NC} build/ supprimé"
fi

# Nettoyer le target Rust
if [ -d "$PROJECT_ROOT/kernel/target" ]; then
    echo -e "${YELLOW}[CLEAN]${NC} Suppression du répertoire kernel/target/..."
    rm -rf "$PROJECT_ROOT/kernel/target"
    echo -e "${GREEN}[SUCCESS]${NC} kernel/target/ supprimé"
fi

# Nettoyer les fichiers objets temporaires
find "$PROJECT_ROOT" -name "*.o" -type f -delete 2>/dev/null || true
find "$PROJECT_ROOT" -name "*.bin" -type f -delete 2>/dev/null || true

echo -e "${BLUE}=========================================${NC}"
echo -e "${GREEN}[SUCCESS]${NC} Nettoyage terminé"
echo -e "${BLUE}=========================================${NC}"
