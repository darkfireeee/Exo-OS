#!/bin/bash
# run-qemu-debug.sh
# Script pour lancer QEMU en mode debug avec affichage VGA

set -e

# Déterminer le répertoire racine du projet
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_ROOT/build"
ISO_FILE="$BUILD_DIR/exo-os.iso"

echo "========================================="
echo "  Exo-OS QEMU Debug Mode"
echo "========================================="

# Vérifier que l'ISO existe
if [ ! -f "$ISO_FILE" ]; then
    echo "[ERROR] ISO non trouvée: $ISO_FILE"
    exit 1
fi

echo "[QEMU] Lancement avec affichage VGA (pas de serial stdio)"
echo "[INFO] ISO: $ISO_FILE"
echo "[INFO] Les marqueurs debug devraient apparaître à l'écran:"
echo "        AA BB PP (mode 32-bit)"
echo "        64SC (mode 64-bit)"
echo "        XXXXXX... (rust_main s'exécute)"
echo "========================================="

# Lancer QEMU SANS -serial stdio pour voir la sortie VGA
qemu-system-x86_64 \
    -cdrom "$ISO_FILE" \
    -boot d \
    -m 512M \
    -no-reboot \
    -no-shutdown

echo "========================================="
echo "[QEMU] Session terminée"
echo "========================================="
