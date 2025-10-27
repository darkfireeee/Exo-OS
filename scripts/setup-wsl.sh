#!/bin/bash
# setup-wsl.sh
# Script d'installation des dépendances pour Exo-OS sous WSL Ubuntu

set -e

# Couleurs
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Exo-OS - Setup WSL Environment${NC}"
echo -e "${BLUE}=========================================${NC}"

# Vérifier qu'on est sous WSL
if ! grep -qi microsoft /proc/version; then
    echo -e "${RED}[ERROR]${NC} Ce script doit être exécuté sous WSL"
    exit 1
fi

echo -e "${YELLOW}[INFO]${NC} Détection de l'environnement WSL..."

# Mise à jour du système
echo -e "${YELLOW}[SETUP]${NC} Mise à jour du système..."
sudo apt-get update -qq

# Installation des outils de build
echo -e "${YELLOW}[SETUP]${NC} Installation des outils de build..."
sudo apt-get install -y -qq \
    build-essential \
    nasm \
    grub-pc-bin \
    grub-common \
    xorriso \
    mtools \
    qemu-system-x86 \
    curl \
    git

echo -e "${GREEN}[SUCCESS]${NC} Outils de build installés"

# Installation de Rust
if ! command -v rustup &> /dev/null; then
    echo -e "${YELLOW}[SETUP]${NC} Installation de Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
    source "$HOME/.cargo/env"
    echo -e "${GREEN}[SUCCESS]${NC} Rust installé"
else
    echo -e "${GREEN}[INFO]${NC} Rust déjà installé"
fi

# S'assurer que cargo est dans le PATH
if ! command -v cargo &> /dev/null; then
    echo -e "${YELLOW}[INFO]${NC} Ajout de Rust au PATH..."
    source "$HOME/.cargo/env"
fi

# Installation de Rust nightly
echo -e "${YELLOW}[SETUP]${NC} Installation de Rust nightly..."
rustup install nightly
rustup default nightly

# Installation de rust-src
echo -e "${YELLOW}[SETUP]${NC} Installation de rust-src..."
rustup component add rust-src

echo -e "${GREEN}[SUCCESS]${NC} Rust nightly configuré"

# Vérification des versions
echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Versions installées${NC}"
echo -e "${BLUE}=========================================${NC}"
echo -e "  NASM:    $(nasm -v)"
echo -e "  GRUB:    $(grub-mkrescue --version | head -1)"
echo -e "  QEMU:    $(qemu-system-x86_64 --version | head -1)"
echo -e "  Rust:    $(rustc --version)"
echo -e "  Cargo:   $(cargo --version)"
echo -e "${BLUE}=========================================${NC}"

echo -e "${GREEN}[SUCCESS]${NC} Environnement configuré avec succès!"
echo -e "${BLUE}=========================================${NC}"
echo -e "Pour compiler Exo-OS:"
echo -e "  ${YELLOW}./scripts/build-iso.sh${NC}"
echo -e ""
echo -e "Pour lancer dans QEMU:"
echo -e "  ${YELLOW}./scripts/run-qemu.sh${NC}"
echo -e "${BLUE}=========================================${NC}"
