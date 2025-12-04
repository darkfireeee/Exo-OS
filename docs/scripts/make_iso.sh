#!/bin/bash
# Script pour créer une ISO bootable avec GRUB

set -e

echo "=== Création d'une ISO bootable ==="

# Vérifier que l'image kernel existe
if [ ! -f "build/kernel.bin" ]; then
    echo "Erreur: build/kernel.bin introuvable"
    exit 1
fi

# Créer la structure de répertoires pour l'ISO
mkdir -p build/iso/boot/grub

# Copier le kernel
cp build/kernel.bin build/iso/boot/

# Copier le fichier grub.cfg depuis bootloader/
if [ -f "bootloader/grub.cfg" ]; then
    cp bootloader/grub.cfg build/iso/boot/grub/
else
    # Fallback: créer un grub.cfg basique
    cat > build/iso/boot/grub/grub.cfg << 'EOF'
set timeout=5
set default=0

menuentry "Exo-OS v0.4.0" {
    multiboot2 /boot/kernel.bin
    boot
}
EOF
fi

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
