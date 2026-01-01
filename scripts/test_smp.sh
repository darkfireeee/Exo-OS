#!/bin/bash
# Script de test SMP pour Exo-OS
# Teste différentes configurations QEMU pour le support SMP

set -e

ISO_PATH="build/exo_os.iso"
DEBUG_LOG="/tmp/exo_smp_debug.log"

echo "═══════════════════════════════════════════════════════"
echo "  Test SMP - Exo-OS"
echo "═══════════════════════════════════════════════════════"
echo ""

# Vérifier que l'ISO existe
if [ ! -f "$ISO_PATH" ]; then
    echo "❌ ISO non trouvée : $ISO_PATH"
    echo "   Exécutez: bash docs/scripts/build.sh"
    exit 1
fi

# Vérifier si KVM est disponible
if [ -c /dev/kvm ]; then
    echo "✓ KVM disponible - Utilisation de la virtualisation matérielle"
    KVM_OPTS="-enable-kvm -cpu host"
    USE_KVM=1
else
    echo "⚠️  KVM non disponible - Utilisation de TCG (émulation)"
    echo "   AVERTISSEMENT: TCG ne supporte PAS correctement le SMP"
    echo "   Les APs risquent de ne pas démarrer (limitation QEMU TCG)"
    KVM_OPTS="-cpu qemu64"
    USE_KVM=0
fi

echo ""
echo "Configuration:"
echo "  - CPU: ${KVM_OPTS}"
echo "  - SMP: 4 cores"
echo "  - RAM: 128M"
echo "  - Debug: Port 0xE9 → $DEBUG_LOG"
echo ""

# Nettoyer l'ancien log
rm -f "$DEBUG_LOG"

echo "Démarrage de QEMU..."
echo "  (Appuyez sur Ctrl+C pour arrêter)"
echo ""

# Options QEMU optimisées pour SMP
QEMU_OPTS=(
    $KVM_OPTS
    -smp 4
    -m 128M
    -cdrom "$ISO_PATH"
    -serial stdio
    -no-reboot
    -debugcon file:"$DEBUG_LOG"
    -d cpu_reset
)

# Si KVM n'est pas disponible, ajouter plus de debug
if [ $USE_KVM -eq 0 ]; then
    QEMU_OPTS+=(-d int,cpu_reset)
fi

# Lancer QEMU
timeout 60 qemu-system-x86_64 "${QEMU_OPTS[@]}" 2>&1 | tee /tmp/exo_smp_serial.log

echo ""
echo "═══════════════════════════════════════════════════════"
echo "  Analyse des Résultats"
echo "═══════════════════════════════════════════════════════"
echo ""

# Vérifier le debug log
if [ -f "$DEBUG_LOG" ] && [ -s "$DEBUG_LOG" ]; then
    echo "✓ Debug output détecté sur port 0xE9:"
    cat "$DEBUG_LOG"
    echo ""
    echo "✓ L'AP a exécuté du code trampoline !"
else
    echo "❌ Aucun output sur port 0xE9"
    echo "   L'AP n'a pas exécuté le code trampoline"
    
    if [ $USE_KVM -eq 0 ]; then
        echo ""
        echo "   CAUSE PROBABLE: QEMU TCG ne supporte pas le SMP"
        echo "   SOLUTION: Tester avec KVM ou sur hardware réel"
    fi
fi

echo ""

# Chercher des messages d'AP online
if grep -q "AP.*online" /tmp/exo_smp_serial.log; then
    echo "✅ SMP FONCTIONNE ! APs démarrés avec succès"
elif grep -q "Triple fault" /tmp/exo_smp_serial.log; then
    echo "❌ Triple fault détecté"
    echo "   Les APs ont crashé pendant le boot"
else
    echo "⚠️  Aucun message d'AP online détecté"
fi

echo ""
echo "Logs complets:"
echo "  - Serial: /tmp/exo_smp_serial.log"
echo "  - Debug:  $DEBUG_LOG"
echo ""
