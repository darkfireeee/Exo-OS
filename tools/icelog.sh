#!/bin/bash
source ~/.profile 2>/dev/null; export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/c/Users/xavie/Desktop/Exo-OS || exit 1
find kernel/src -name '*.rs' -exec touch {} + 2>/dev/null
export RUST_BACKTRACE=0
make iso >/tmp/mk.log 2>&1
echo "=== exit=$? ==="
grep -iE "panic|internal compiler|thread 'rustc'|query stack|^error|deeply nested|stack" /tmp/mk.log | head -40
echo "=== contexte autour du panic ==="
grep -n -A2 -B2 -iE "panicked|internal compiler error" /tmp/mk.log | head -40
