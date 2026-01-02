#!/bin/bash
# Test rapide SMP avec capture des logs

LOG_FILE="/tmp/exo_kernel_output.log"
rm -f "$LOG_FILE"

echo "Starting QEMU with SMP support..."
echo "Log file: $LOG_FILE"
echo ""

# Lancer QEMU en arrière-plan et capturer la sortie
timeout 20 qemu-system-x86_64 \
    -m 256M \
    -smp 4 \
    -cdrom build/exo_os.iso \
    -serial file:"$LOG_FILE" \
    -display none \
    -no-reboot \
    2>/dev/null

# Attendre que QEMU se termine
sleep 1

# Afficher les résultats
if [ -f "$LOG_FILE" ]; then
    echo "═══════════════════════════════════════════════════════"
    echo "  KERNEL OUTPUT"
    echo "═══════════════════════════════════════════════════════"
    cat "$LOG_FILE"
    echo ""
    echo "═══════════════════════════════════════════════════════"
    
    # Analyser les résultats des tests
    if grep -q "PHASE 2b" "$LOG_FILE"; then
        echo ""
        echo "✅ Tests SMP trouvés dans la sortie!"
        echo ""
        grep -A 50 "PHASE 2b" "$LOG_FILE" | head -100
    else
        echo "⚠️  Tests SMP non trouvés - kernel peut avoir crash avant"
    fi
else
    echo "❌ Aucun fichier de log généré"
fi
