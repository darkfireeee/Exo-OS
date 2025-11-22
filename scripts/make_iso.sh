#!/bin/bash
# Script pour créer une ISO bootable avec GRUB

set -e

echo "=== Création d'une ISO bootable ==="

# Vérifier que l'image kernel existe
if [ ! -f "build/exo_kernel.elf" ]; then
    echo "Erreur: build/exo_kernel.elf introuvable"
    exit 1
fi

# Créer la structure de répertoires pour l'ISO
mkdir -p build/iso/boot/grub

# Copier le kernel
cp build/exo_kernel.elf build/iso/boot/

# Créer le fichier de configuration GRUB
cat > build/iso/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0

menuentry "Exo-OS" {
    multiboot /boot/exo_kernel.elf
    boot
}
EOF

echo "-> Structure ISO créée"

# Vérifier que grub-mkrescue est disponible
if ! command -v grub-mkrescue &> /dev/null; then
    echo "ATTENTION: grub-mkrescue non trouvé"
    echo "Installez avec: sudo apt install grub-pc-bin xorriso"
    echo ""
    echo "Structure ISO créée dans build/iso/"
    echo "Vous pouvez créer l'ISO manuellement avec:"
    echo "  grub-mkrescue -o build/exo_os.iso build/iso/"
    exit 0
fi

# Créer l'ISO avec GRUB
echo "-> Création de l'ISO avec GRUB..."
grub-mkrescue -o build/exo_os.iso build/iso/ 2>&1 | grep -v "WARNING:"

echo "-> ISO créée: build/exo_os.iso"
ls -lh build/exo_os.iso

echo ""
echo "=== ISO bootable créée avec succès ==="
