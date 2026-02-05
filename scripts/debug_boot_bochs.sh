#!/bin/bash
# Debug boot avec Bochs - analyse détaillée des erreurs multiboot

set -e

echo "=== Bochs Boot Debug - Exo-OS ==="
echo ""

# Vérifier ISO
if [ ! -f "build/exo_os.iso" ]; then
    echo "❌ ISO non trouvée: build/exo_os.iso"
    exit 1
fi

echo "✅ ISO: $(ls -lh build/exo_os.iso | awk '{print $5}')"

# Créer config Bochs pour debug boot (nogui + logs verbeux)
cat > /tmp/bochsrc_boot_debug.txt << 'EOF'
# Bochs Boot Debug Config
cpu: count=1, ips=5000000, reset_on_triple_fault=0
memory: guest=128, host=128

# Boot depuis ISO
ata0-master: type=cdrom, path="build/exo_os.iso", status=inserted
boot: cdrom

# Mode term pour capture
display_library: term
vga: extension=vbe, update_freq=5

# Logs détaillés
log: /tmp/bochs.log
debugger_log: /tmp/bochs_debug.log
panic: action=fatal
error: action=fatal
info: action=report
debug: action=ignore

# Port E9 pour output kernel
port_e9_hack: enabled=1

clock: sync=realtime
magic_break: enabled=1
EOF

echo "✅ Config debug créée"
echo ""

# Nettoyer logs
rm -f /tmp/bochs.log /tmp/bochs_debug.log

echo "🚀 Lancement Bochs (timeout 10s)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd /workspaces/Exo-OS

# Lancer avec timeout
timeout 10 bochs -f /tmp/bochsrc_boot_debug.txt -q 2>&1 || {
    CODE=$?
    echo ""
    if [ $CODE -eq 124 ]; then
        echo "⏱️  Timeout (boot peut avoir réussi)"
    else
        echo "❌ Bochs exit code: $CODE"
    fi
}

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📋 ANALYSE DES LOGS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ -f /tmp/bochs.log ]; then
    # Chercher les erreurs multiboot/boot
    echo "🔍 Recherche erreurs boot:"
    echo ""
    
    if grep -qi "multiboot" /tmp/bochs.log; then
        echo "  ✅ Mention 'multiboot' trouvée dans les logs"
        grep -i "multiboot" /tmp/bochs.log | head -5
    else
        echo "  ⚠️  Pas de mention 'multiboot'"
    fi
    
    echo ""
    if grep -qi "magic\|invalid\|error\|panic\|fail" /tmp/bochs.log; then
        echo "  ❌ Erreurs détectées:"
        grep -i "magic\|invalid\|error\|panic\|fail" /tmp/bochs.log | head -10
    else
        echo "  ✅ Pas d'erreur critique détectée"
    fi
    
    echo ""
    echo "📄 Dernières 40 lignes du log:"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    tail -40 /tmp/bochs.log
else
    echo "⚠️  Aucun log Bochs généré (/tmp/bochs.log)"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Logs disponibles in:"
echo "  • /tmp/bochs.log (principal)"
echo "  • /tmp/bochs_debug.log (debug)"
