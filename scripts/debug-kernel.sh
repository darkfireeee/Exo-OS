#!/bin/bash
# Test de debug du kernel Exo-OS avec output détaillé

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_PATH="$PROJECT_ROOT/build/exo-os.iso"

echo "🔍 Debug Test du kernel Exo-OS"
echo "📁 ISO: $ISO_PATH"
echo ""

# Vérifier que l'ISO existe
if [ ! -f "$ISO_PATH" ]; then
    echo "❌ ISO non trouvée: $ISO_PATH"
    echo "💡 Compilez d'abord avec: ./scripts/build-iso.sh"
    exit 1
fi

echo "✅ ISO trouvée ($(du -h "$ISO_PATH" | cut -f1))"
echo ""

# Créer un fichier de sortie
OUTPUT_FILE="/tmp/exo_kernel_output.log"
rm -f "$OUTPUT_FILE"

echo "🔧 Configuration QEMU:"
echo "   - Sortie série: $OUTPUT_FILE"
echo "   - Mode: nographic (sortie VGA seulement)"
echo "   - Mémoire: 1GB"
echo ""

echo "🚀 Lancement QEMU (timeout 10 secondes)..."
echo "💡 Si ça marche, vous devriez voir 'Exo-OS' sur l'écran VGA"
echo ""

# Test 1: Mode graphique avec capture série
echo "=== Test 1: Mode graphique avec log série ==="
timeout 10 qemu-system-x86_64 \
    -cdrom "$ISO_PATH" \
    -m 1G \
    -serial file:"$OUTPUT_FILE" \
    -display gtk \
    -no-reboot \
    -no-hpet \
    -machine hpet=off \
    -debugcon stdio 2>&1 | tee /tmp/qemu_stderr.log || echo "⏹️ QEMU terminé (test 1)"

echo ""
echo "📊 Résultats du Test 1:"
if [ -f "$OUTPUT_FILE" ] && [ -s "$OUTPUT_FILE" ]; then
    echo "✅ Fichier de sortie série créé ($(wc -l < "$OUTPUT_FILE") lignes)"
    echo "📄 Contenu:"
    cat "$OUTPUT_FILE"
else
    echo "❌ Aucune sortie série capturée"
fi

echo ""
echo "🔍 Analyse des logs stderr:"
if [ -f "/tmp/qemu_stderr.log" ]; then
    grep -i "exo\|kernel\|boot\|panic\|error" /tmp/qemu_stderr.log || echo "Aucun pattern trouvé dans stderr"
fi

# Test 2: Mode console pure (sans affichage graphique)
echo ""
echo "=== Test 2: Mode console pure ==="
OUTPUT_FILE2="/tmp/exo_kernel_output2.log"
rm -f "$OUTPUT_FILE2"

timeout 8 qemu-system-x86_64 \
    -cdrom "$ISO_PATH" \
    -m 512M \
    -serial stdio \
    -nographic \
    -no-reboot \
    -machine hpet=off \
    2>&1 | tee /tmp/qemu_stderr2.log || echo "⏹️ QEMU terminé (test 2)"

echo ""
echo "📊 Résultats du Test 2:"
echo "📄 Capture stdout/stderr:"
cat /tmp/qemu_stderr2.log

echo ""
echo "🔍 Analyse finale:"
echo "💡 Prochaines étapes:"
echo "   1. Si vous voyez 'Exo-OS' à l'écran: Le kernel fonctionne ✅"
echo "   2. Si sortie série vide: Problème de configuration serial"
echo "   3. Si panic dans logs: Problème de code kernel"
echo ""
echo "📋 Fichiers de log générés:"
echo "   - $OUTPUT_FILE (test 1)"
echo "   - /tmp/qemu_stderr.log (test 1)"
echo "   - /tmp/qemu_stderr2.log (test 2)"