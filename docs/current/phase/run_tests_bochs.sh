#!/bin/bash
# Test automatique SMP avec Bochs - Capture des résultats
# Bochs supporte mieux le SMP que QEMU TCG

set -e

LOG_FILE="/tmp/bochs_test_output.log"
DEBUG_FILE="/tmp/bochs_debug.log"
SERIAL_FILE="/tmp/bochs_serial.log"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Exo-OS v0.6.0 - SMP Tests avec Bochs (4 CPUs)              ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# Vérifications
if ! command -v bochs &> /dev/null; then
    echo "❌ Bochs non installé"
    exit 1
fi

if [ ! -f "build/exo_os.iso" ]; then
    echo "❌ ISO non trouvée. Exécutez: bash docs/scripts/build.sh"
    exit 1
fi

# Nettoyer les anciens logs
rm -f "$LOG_FILE" "$DEBUG_FILE" "$SERIAL_FILE"

echo "Configuration:"
echo "  - Bochs: $(bochs --help 2>&1 | head -1)"
echo "  - CPUs: 4 cores (SMP)"
echo "  - RAM: 128MB"
echo "  - ISO: build/exo_os.iso"
echo ""

# Créer une configuration Bochs temporaire avec port série
cat > /tmp/bochsrc_test.txt << 'EOF'
# Configuration test automatique
cpu: count=4, ips=100000000, reset_on_triple_fault=0, ignore_bad_msrs=1
memory: guest=128, host=128
ata0-master: type=cdrom, path="build/exo_os.iso", status=inserted
boot: cdrom

# Output
display_library: term, options="hideIPS"
vga: extension=vbe

# Logging
log: /tmp/bochs_test_output.log
debugger_log: /tmp/bochs_debug.log
panic: action=report
error: action=report
info: action=report
debug: action=ignore

# Port E9 pour les logs kernel
port_e9_hack: enabled=1

# Serial port (capture les early_print)
com1: enabled=1, mode=file, dev=/tmp/bochs_serial.log

# Pas de clavier/souris (mode headless)
keyboard: enabled=0
mouse: enabled=0

# Clock
clock: sync=realtime
magic_break: enabled=0
EOF

echo "Démarrage de Bochs (timeout 60s)..."
echo ""

# Lancer Bochs en arrière-plan avec timeout
# Utiliser 'c' pour continuer automatiquement
echo "c" | timeout 60 bochs -f /tmp/bochsrc_test.txt -q 2>&1 | tee /tmp/bochs_stdout.log &
BOCHS_PID=$!

# Attendre que Bochs démarre
sleep 5

# Monitorer les logs en temps réel
echo "═══════════════════════════════════════════════════════════════"
echo "  KERNEL OUTPUT (Port E9 / Serial)"
echo "═══════════════════════════════════════════════════════════════"
echo ""

# Attendre jusqu'à 50 secondes pour les résultats
for i in {1..50}; do
    sleep 1
    
    # Vérifier si les tests sont apparus
    if [ -f "$SERIAL_FILE" ] && grep -q "PHASE 2b" "$SERIAL_FILE" 2>/dev/null; then
        echo "✅ Tests détectés! Attente de la fin..."
        sleep 10  # Attendre que tous les tests se terminent
        break
    fi
    
    # Afficher un point tous les 5 secondes
    if [ $((i % 5)) -eq 0 ]; then
        echo "⏳ Waiting... ($i/50s)"
    fi
done

# Tuer Bochs
kill $BOCHS_PID 2>/dev/null || true
sleep 1

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  ANALYSE DES RÉSULTATS"
echo "═══════════════════════════════════════════════════════════════"
echo ""

# Combiner tous les fichiers de log
cat "$SERIAL_FILE" "$LOG_FILE" 2>/dev/null | grep -v "^\[" | grep -v "^Bochs" > /tmp/combined_output.log || true

# Afficher la sortie
if [ -s /tmp/combined_output.log ]; then
    cat /tmp/combined_output.log
    echo ""
    
    # Analyser les résultats
    if grep -q "PHASE 2b" /tmp/combined_output.log; then
        echo "✅ Tests SMP trouvés!"
        echo ""
        
        # Compter les PASS/FAIL
        PASS_COUNT=$(grep -c "PASS" /tmp/combined_output.log || echo "0")
        FAIL_COUNT=$(grep -c "FAIL" /tmp/combined_output.log || echo "0")
        
        echo "📊 Résumé:"
        echo "  ✅ PASS: $PASS_COUNT"
        echo "  ❌ FAIL: $FAIL_COUNT"
        
        if [ "$FAIL_COUNT" -eq "0" ] && [ "$PASS_COUNT" -gt "0" ]; then
            echo ""
            echo "🎉 SUCCÈS - Tous les tests passent!"
        fi
    else
        echo "⚠️  Tests SMP non trouvés dans la sortie"
        echo "    Le kernel a peut-être crash avant les tests"
    fi
else
    echo "❌ Aucune sortie capturée"
    echo ""
    echo "Logs disponibles:"
    echo "  - $LOG_FILE"
    echo "  - $DEBUG_FILE"
    echo "  - $SERIAL_FILE"
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "Logs sauvegardés:"
echo "  - Combined: /tmp/combined_output.log"
echo "  - Serial: $SERIAL_FILE"
echo "  - Bochs: $LOG_FILE"
echo "═══════════════════════════════════════════════════════════════"
