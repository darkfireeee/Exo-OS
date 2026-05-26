#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
cd "$REPO_ROOT"

DISK_IMAGE=${1:-target/qemu/exofs-root.img}
ISO_IMAGE=${2:-exo-os.iso}
E9_LOG=${EXOOS_E9_LOG:-/tmp/e9k.txt}
INT_LOG=${EXOOS_INT_LOG:-/tmp/qemu-exoos.log}
SERIAL=${EXOOS_SERIAL:-stdio}
NET_FLAGS=(
  -netdev user,id=exovirtio
  -device virtio-net-pci-non-transitional,netdev=exovirtio,mac=02:45:58:4f:00:01
  -netdev user,id=exoe1000
  -device e1000,netdev=exoe1000,mac=02:45:58:4f:00:02
)

set +e
qemu-system-x86_64 \
  -machine q35 \
  -m 256M \
  -boot d \
  -vga std \
  -serial "$SERIAL" \
  -no-reboot \
  -no-shutdown \
  -d int,cpu_reset -D "$INT_LOG" \
  -debugcon file:"$E9_LOG" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -drive if=none,file="$DISK_IMAGE",format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  "${NET_FLAGS[@]}" \
  -cdrom "$ISO_IMAGE"
status=$?
set -e

if [[ "$status" -eq 33 ]]; then
  exit 0
fi
exit "$status"
