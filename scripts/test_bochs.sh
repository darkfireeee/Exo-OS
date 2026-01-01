#!/bin/bash
# Test SMP avec Bochs
# Bochs émule mieux le SMP que QEMU TCG

set -e

echo "═══════════════════════════════════════════════════════"
echo "  Test SMP avec Bochs - Exo-OS"
echo "═══════════════════════════════════════════════════════"
echo ""

# Vérifier que Bochs est installé
if ! command -v bochs &> /dev/null; then
    echo "❌ Bochs n'est pas installé"
    echo "   Installez-le avec: sudo apk add bochs"
    echo "   Ou compilez-le depuis les sources"
    exit 1
fi

# Vérifier que l'ISO existe
if [ ! -f "build/exo_os.iso" ]; then
    echo "❌ ISO non trouvée : build/exo_os.iso"
    echo "   Exécutez: bash docs/scripts/build.sh"
    exit 1
fi

# Vérifier que bochsrc.txt existe
if [ ! -f "bochsrc.txt" ]; then
    echo "❌ Configuration Bochs non trouvée : bochsrc.txt"
    exit 1
fi

echo "Configuration:"
echo "  - Émulateur: Bochs $(bochs --help 2>&1 | grep -oP 'Bochs x86 Emulator \K[0-9.]+')"
echo "  - CPU: 4 cores (SMP)"
echo "  - RAM: 128M"
echo "  - ISO: build/exo_os.iso"
echo "  - Logs: /tmp/bochs.log, /tmp/bochs_debug.log"
echo ""

# Nettoyer les anciens logs
rm -f /tmp/bochs.log /tmp/bochs_debug.log

echo "Démarrage de Bochs..."
echo "  (Tapez 'c' puis Enter pour continuer après l'écran de config)"
echo "  (Ctrl+C pour arrêter)"
echo ""

# Lancer Bochs
bochs -f bochsrc.txt -q

echo ""
echo "═══════════════════════════════════════════════════════"
echo "  Analyse des Résultats"
echo "═══════════════════════════════════════════════════════"
echo ""

# Vérifier les logs
if grep -q "AP.*online" /tmp/bochs.log 2>/dev/null; then
    echo "✅ SMP FONCTIONNE ! APs démarrés avec succès"
elif grep -q "Triple fault" /tmp/bochs.log 2>/dev/null; then
    echo "❌ Triple fault détecté"
    echo "   Les APs ont crashé pendant le boot"
elif grep -q "XYZ" /tmp/bochs.log 2>/dev/null; then
    echo "✓ Trampoline minimal exécuté (marqueurs XYZ détectés)"
    echo "⚠️  Mais APs pas encore online - vérifier le code 64-bit"
else
    echo "⚠️  Résultats non concluants"
    echo "   Vérifiez les logs manuellement"
fi

echo ""
echo "Logs disponibles:"
echo "  - Bochs: /tmp/bochs.log"
echo "  - Debug: /tmp/bochs_debug.log"
echo ""
echo "Pour voir les détails:"
echo "  cat /tmp/bochs.log | less"
echo ""
