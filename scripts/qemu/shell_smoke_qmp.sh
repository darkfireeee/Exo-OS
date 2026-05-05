#!/usr/bin/env bash
set -euo pipefail

DISK_IMAGE=${1:-target/qemu/exofs-root.img}
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
cd "$REPO_ROOT"

QMP=/tmp/exoos-shell-qmp.sock
SERIAL=/tmp/exoos-shell-serial.log
E9=/tmp/exoos-shell-e9.log
INTLOG=/tmp/exoos-shell-int.log
STDOUT=/tmp/exoos-shell-qemu.log
PIDFILE=/tmp/exoos-shell-qemu.pid

pkill -f "qemu-system-x86_64.*exoos-shell-qmp" 2>/dev/null || true
rm -f "$QMP" "$SERIAL" "$E9" "$INTLOG" "$STDOUT" "$PIDFILE"

qemu-system-x86_64 \
  -machine q35 \
  -m 256M \
  -boot d \
  -vga std \
  -serial file:"$SERIAL" \
  -no-reboot \
  -no-shutdown \
  -monitor none \
  -display none \
  -d int,cpu_reset -D "$INTLOG" \
  -debugcon file:"$E9" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -qmp unix:"$QMP",server=on,wait=off \
  -drive if=none,file="$DISK_IMAGE",format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0 \
  -cdrom exo-os.iso \
  >"$STDOUT" 2>&1 &
echo $! >"$PIDFILE"

cleanup() {
  if [[ -f "$PIDFILE" ]]; then
    kill "$(cat "$PIDFILE")" 2>/dev/null || true
  fi
}
trap cleanup EXIT

for _ in $(seq 1 100); do
  [[ -S "$QMP" ]] && break
  sleep 0.2
done
[[ -S "$QMP" ]] || { cat "$STDOUT"; exit 1; }

python3 - "$QMP" <<'PY'
import json
import socket
import sys
import time

qmp = sys.argv[1]
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(qmp)
sock.recv(65536)
sock.sendall((json.dumps({"execute": "qmp_capabilities"}) + "\r\n").encode())
sock.recv(65536)

def hmp(command):
    sock.sendall((json.dumps({
        "execute": "human-monitor-command",
        "arguments": {"command-line": command},
    }) + "\r\n").encode())
    return sock.recv(65536)

time.sleep(12)
for text in [
    "pwd\n",
    "mkdir /tmp/t\n",
    "touch /tmp/t/a\n",
    "echo hi > /tmp/t/a\n",
    "cat /tmp/t/a\n",
    "ls /tmp/t\n",
    "rm /tmp/t/a\n",
    "rmdir /tmp/t\n",
]:
    for ch in text:
        hmp("sendkey " + ("ret" if ch == "\n" else ch))
        time.sleep(0.03)

hmp("screendump /tmp/exoos-shell-screen.ppm")
sock.close()
PY

echo "SERIAL=$SERIAL"
echo "E9=$E9"
echo "SCREEN=/tmp/exoos-shell-screen.ppm"
