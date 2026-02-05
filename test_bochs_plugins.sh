#!/bin/bash
# Test Bochs avec config explicite des plugins

cd /workspaces/Exo-OS

echo "=== Test Boot Bochs avec Plugins Explicites ==="

# Config avec plugins explicites
cat > /tmp/bochs_plugins.rc << 'EOF'
# Config Bochs avec plugins explicites
plugin_ctrl: unmapped=1, biosdev=1, speaker=1, extfpuirq=1, parallel=1, serial=1, gameport=1, iodebug=1
plugin_ctrl: pci=1, pci2isa=1, usb_uhci=0, usb_ohci=0, usb_xhci=0
plugin_ctrl: e1000=0, ne2k=0
plugin_ctrl:busmouse=0
plugin_ctrl: vga=1, svga_cirrus=0, voodoo=0

# CPU & Memory
cpu: count=1, ips=5000000, reset_on_triple_fault=0
memory: guest=128, host=128

# Devices IDE/ATA
ata0: enabled=1, ioaddr1=0x1f0, ioaddr2=0x3f0, irq=14
ata1: enabled=0
ata2: enabled=0
ata3: enabled=0

# CDROM sur ATA0 master
ata0-master: type=cdrom, path="/workspaces/Exo-OS/build/exo_os.iso", status=inserted

# Boot
boot: cdrom

# Display
display_library: term

# Logs
log: /tmp/bochs_plugins.log
panic: action=report
error: action=report  
info: action=report

# Debug output
port_e9_hack: enabled=1
EOF

echo "✅ Config avec plugins créée"
echo ""

rm -f /tmp/bochs_plugins.log

echo "🚀 Lancement Bochs (7s timeout)..."

(timeout 7 bochs -f /tmp/bochs_plugins.rc -q 2>&1 || true) &
BOCHS_PID=$!

sleep 6
kill -9 $BOCHS_PID 2>/dev/null || true
sleep 1

echo ""
echo "=== RÉSULTATS ==="
echo ""

if [ ! -f /tmp/bochs_plugins.log ]; then
    echo "❌ Log non créé"
    exit 1
fi

LINES=$(wc -l < /tmp/bochs_plugins.log)
echo "📊 Log: $LINES lignes"
echo ""

echo "🔍 Plugins initialisés:"
grep -i "PLUGIN.*init\|ATA\|ATAPI\|HD\|CD-ROM" /tmp/bochs_plugins.log | head -15

echo ""
echo "🔍 tentatives boot:"
grep -i "boot\|grub\|multiboot\|loading" /tmp/bochs_plugins.log | head -10 || echo "  Aucun"

echo ""
echo "🔍 Erreurs:"
grep -i "error\|panic\|fatal" /tmp/bochs_plugins.log | head -5 || echo "  Aucune"

echo ""
echo "📋 Lastières 50 lignes du log:"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
tail -50 /tmp/bochs_plugins.log

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Log: /tmp/bochs_plugins.log"
