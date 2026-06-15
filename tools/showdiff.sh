#!/bin/bash
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
for f in "$@"; do
  echo "========== $f =========="
  git diff -- "$f" | grep -aE '^[-+]' | grep -avE '^(\+\+\+|---)'
  echo ""
done
