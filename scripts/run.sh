#!/bin/bash
# Script tout-en-un : compilation + test

set -e

echo "=========================================="
echo "   Exo-OS - Compilation et Test"
echo "=========================================="
echo ""

# Compiler
./scripts/build.sh

echo ""
read -p "Appuyez sur Entrée pour lancer QEMU (ou Ctrl+C pour annuler)..."
echo ""

# Tester
./scripts/test_qemu.sh
