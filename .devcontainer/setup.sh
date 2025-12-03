#!/bin/bash
# Script de configuration pour Exo-OS Development Environment
set -e

echo "🚀 Configuration de l'environnement Exo-OS..."

# Mise à jour des paquets
echo "📦 Installation des dépendances système..."
sudo apt-get update
sudo apt-get install -y \
    nasm \
    clang \
    lld \
    llvm \
    qemu-system-x86 \
    xorriso \
    grub-pc-bin \
    mtools

# Configuration de Rust
echo "🦀 Configuration de Rust..."
source "$HOME/.cargo/env"

# Ajout des composants Rust nécessaires
rustup component add rust-src rustfmt clippy llvm-tools-preview
rustup target add x86_64-unknown-none

# Installation de bootimage
echo "📀 Installation de bootimage..."
cargo install bootimage

# Vérification des installations
echo ""
echo "✅ Vérification des outils installés:"
echo "   Rust: $(rustc --version)"
echo "   Cargo: $(cargo --version)"
echo "   NASM: $(nasm --version)"
echo "   Clang: $(clang --version | head -1)"
echo "   LLD: $(ld.lld --version)"
echo "   QEMU: $(qemu-system-x86_64 --version | head -1)"

echo ""
echo "🎉 Environnement Exo-OS prêt !"
echo ""
echo "Commandes utiles:"
echo "   make build    - Compiler le kernel"
echo "   make qemu     - Lancer avec QEMU"
echo "   make help     - Afficher l'aide"
