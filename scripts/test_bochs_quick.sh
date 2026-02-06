#!/bin/bash
# Test boot simple avec Bochs - capture output vers fichier

cd /workspaces/Exo-OS

echo "=== Test Boot Bochs - ISO Corrigée ==="
echo "ISO: $(ls -lh build/exo_os.iso | awk '{print $5}')"

# Config minimale
cat > /tmp/bochs_simple.rc << 'EOF'
cpu: count=1, ips=5000000
memory: guest=128, host=128
ata0-master: type=cdrom, path="/workspaces/Exo-OS/build/exo_os.iso", status=inserted
boot: cdrom
display_library: term
log: /tmp/bochs_simple.log
panic: action=fatal
error: action=report
info: action=report
port_e9_hack: enabled=1
EOF

rm -f /tmp/bochs_simple.log

echo "Lancement Bochs (5s)..."

# Lance dans subshell avec timeout
(
    timeout 5 bochs -f /tmp/bochs_simple.rc -q 2>&1 || true
) &
BOCHS_PID=$!

sleep 4
kill -9 $BOCHS_PID 2>/dev/null || true
sleep 1

echo ""
echo "=== RÉSULTATS ==="
echo ""

if [ ! -f /tmp/bochs_simple.log ]; then
    echo "❌ Pas de log générésleep!"
    exit 1
fi

LOG_LINES=$(wc -l < /tmp/bochs_simple.log)
echo "📊 Log: $LOG_LINES lignes"
echo ""

# Chercher messages importants
echo "🔍 Recherche multiboot/boot:"
grep -i "multiboot\|grub\|loading\|boot" /tmp/bochs_simple.log || echo "  Aucun trouvé"

echo ""
echo "🔍 Recherche erreurs:"
grep -i "error\|panic\|fail" /tmp/bochs_simple.log| head -5 || echo "  Aucune erreur"

echo ""
echo "📋 Dernières 40 lignes:"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
tail -40 /tmp/bochs_simple.log

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Log complet: /tmp/bochs_simple.log"
