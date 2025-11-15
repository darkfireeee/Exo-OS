#!/bin/bash
# test-qemu-memory.sh
# Lance QEMU et dump la mémoire VGA pour voir les marqueurs

cd /mnt/c/Users/Eric/Documents/Exo-OS

echo "Lancement de QEMU avec dump mémoire..."

# Lancer QEMU avec QMP (QEMU Machine Protocol) pour contrôle
timeout 3 qemu-system-x86_64 \
    -cdrom build/exo-os.iso \
    -boot d \
    -m 512M \
    -nographic \
    -monitor none \
    -serial null \
    2>&1 &

QEMU_PID=$!
echo "QEMU PID: $QEMU_PID"

# Attendre que QEMU démarre et que le kernel s'exécute
sleep 2

# Tuer QEMU
kill $QEMU_PID 2>/dev/null

echo "Test terminé"
