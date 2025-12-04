#!/bin/bash
# Script de test du kernel dans QEMU

set -e

echo "=== Test du kernel Exo-OS dans QEMU ==="

# Vérifier que l'ISO existe
if [ ! -f "build/exo_os.iso" ]; then
    echo "Erreur: build/exo_os.iso introuvable"
    echo "Exécutez d'abord ./make_iso.sh"
    exit 1
fi

# Vérifier que QEMU est installé
if ! command -v qemu-system-x86_64 &> /dev/null; then
    echo "Erreur: qemu-system-x86_64 non trouvé"
    echo "Installez QEMU avec: sudo apt install qemu-system-x86"
    exit 1
fi

echo "-> Lancement de QEMU avec affichage graphique..."
echo "   (Appuyez sur Ctrl+Alt+2 pour console, Ctrl+Alt+1 pour revenir)"
echo "   (Fermez la fenêtre pour quitter)"
echo ""

# Lancer QEMU avec l'ISO en mode graphique et traces debug
echo "QEMU lancé avec traces debug sur stdout"
echo "Fermez la fenêtre QEMU pour terminer"
echo ""
echo "=== Traces de boot ==="
qemu-system-x86_64 \
    -cdrom build/exo_os.iso \
    -m 128M \
    -serial stdio \
    -no-reboot \
    -no-shutdown \
    -d cpu_reset

echo ""
echo "=== Test QEMU terminé ==="
