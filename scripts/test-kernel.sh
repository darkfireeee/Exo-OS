#!/bin/bash
# Test simple du kernel Exo-OS pour débogage

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_PATH="$PROJECT_ROOT/build/exo-os.iso"

echo "🚀 Test simple du kernel Exo-OS"
echo "📁 ISO: $ISO_PATH"
echo ""

# Vérifier que l'ISO existe
if [ ! -f "$ISO_PATH" ]; then
    echo "❌ ISO non trouvée: $ISO_PATH"
    echo "💡 Compilez d'abord avec: ./scripts/build-iso.sh"
    exit 1
fi

echo "✅ ISO trouvée"
echo ""

# Lancer QEMU avec options simplifiées
echo "🔧 Lancement QEMU (attendre 20 secondes)..."
echo "Pour arrêter: Ctrl+C"
echo ""

# QEMU avec logging maximum et debug
timeout 20 qemu-system-x86_64 \
    -cdrom "$ISO_PATH" \
    -m 1G \
    -serial stdio \
    -nographic \
    -no-reboot \
    -machine hpet=off \
    -d guest_errors \
    -D /tmp/qemu_debug.log \
    -debugcon stdio 2>&1 || echo "⏹️ QEMU terminé"

echo ""
echo "📊 Analyse des logs..."

# Analyser les logs
if [ -f "/tmp/qemu_debug.log" ]; then
    echo "🔍 Log QEMU debug:"
    grep -i "exo\|kernel\|panic\|error" /tmp/qemu_debug.log | head -10 || echo "Aucun pattern trouvé"
else
    echo "❌ Log de debug non trouvé"
fi

echo ""
echo "💡 Si vous ne voyez pas de sortie du kernel, vérifiez:"
echo "   1. Le kernel démarre correctement"
echo "   2. La sortie série est configurée"
echo "   3. Il n'y a pas de panic fatal"