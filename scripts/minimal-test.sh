#!/bin/bash
# Test minimal du bootloader/kernel Exo-OS

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_PATH="$PROJECT_ROOT/build/exo-os.iso"
KERNEL_BIN="$PROJECT_ROOT/build/kernel.bin"

echo "🧪 Test Minimal Exo-OS"
echo "📁 ISO: $ISO_PATH"
echo "🗂️  Kernel: $KERNEL_BIN"
echo ""

# Vérifier les fichiers
if [ ! -f "$ISO_PATH" ]; then
    echo "❌ ISO non trouvée: $ISO_PATH"
    exit 1
fi

if [ ! -f "$KERNEL_BIN" ]; then
    echo "❌ Kernel binary non trouvé: $KERNEL_BIN"
    exit 1
fi

echo "✅ Fichiers trouvés:"
echo "   ISO: $(du -h "$ISO_PATH" | cut -f1)"
echo "   Kernel: $(du -h "$KERNEL_BIN" | cut -f1)"
echo ""

# Vérifier le format du kernel (multiboot)
echo "🔍 Vérification du format kernel:"
if file "$KERNEL_BIN" | grep -q "ELF"; then
    echo "✅ Kernel est un binaire ELF"
else
    echo "⚠️  Kernel n'est pas un ELF standard"
fi

# Tester avec QEMU en mode debug maximal
echo ""
echo "🔧 Test QEMU avec debug maximal:"
echo "💡 Ce test lance QEMU 5 secondes et capture TOUT"
echo ""

OUTPUT_FILE="/tmp/minimal_test.log"
rm -f "$OUTPUT_FILE"

# QEMU avec tous les debugs activés
timeout 5 qemu-system-x86_64 \
    -cdrom "$ISO_PATH" \
    -m 256M \
    -serial stdio \
    -nographic \
    -no-reboot \
    -machine hpet=off \
    -d guest_errors \
    -d exec \
    -D "$OUTPUT_FILE" \
    2>&1 || echo "⏹️ QEMU terminé"

echo ""
echo "📊 Résultats:"

if [ -s "$OUTPUT_FILE" ]; then
    echo "✅ Log de debug généré ($(wc -l < "$OUTPUT_FILE") lignes)"
    echo ""
    echo "🔍 Analyse du log (lignes importantes):"
    grep -i "multiboot\|kernel\|entry\|error\|panic" "$OUTPUT_FILE" | head -10 || echo "   Aucun pattern trouvé"
    
    echo ""
    echo "📄 20 premières lignes du log:"
    head -20 "$OUTPUT_FILE"
else
    echo "❌ Aucun log de debug généré"
fi

# Test alternatif: lancer directement le kernel (sans ISO)
echo ""
echo "🧪 Test direct du kernel (sans bootloader):"
timeout 3 qemu-system-x86_64 \
    -kernel "$KERNEL_BIN" \
    -m 256M \
    -serial stdio \
    -nographic \
    -no-reboot \
    -machine hpet=off \
    2>&1 | tee /tmp/direct_kernel.log || echo "⏹️ Test direct terminé"

if [ -s "/tmp/direct_kernel.log" ]; then
    echo "✅ Sortie du test direct:"
    cat /tmp/direct_kernel.log
else
    echo "❌ Aucune sortie du test direct"
fi

echo ""
echo "💡 Conclusion:"
echo "   - Si aucune sortie: Problème de boot/multiboot"
echo "   - Si panic visible: Problème dans le code kernel"  
echo "   - Si 'Hello': Kernel fonctionne"
echo ""
echo "📋 Logs générés:"
echo "   - $OUTPUT_FILE (debug complet)"
echo "   - /tmp/direct_kernel.log (test direct)"