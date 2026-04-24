set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
export PATH="$HOME/.cargo/bin:$PATH"
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
OUTPUT_DIR="$REPO_ROOT/docs/avancement/qemu_boot"
OUT_PPM="$OUTPUT_DIR/exoos-qemu-latest.ppm"
OUT_PNG="$OUTPUT_DIR/exoos-qemu-latest.png"
mkdir -p "$OUTPUT_DIR"
cd "$REPO_ROOT"
pkill -f "qemu-system-x86_64.*exo-os.iso" 2>/dev/null || true
rm -f /tmp/exoos-qmp.sock /tmp/exoos-qemu.pid /tmp/exoos-serial.log /tmp/exoos-e9.log /tmp/exoos-qemu-int.log /tmp/exoos-qemu-stdout.log "$OUT_PPM" "$OUT_PNG"
if ! make iso >/tmp/exoos-make.log 2>&1; then
  cat /tmp/exoos-make.log
  exit 1
fi
nohup qemu-system-x86_64 \
  -machine q35 \
  -m 256M \
  -vga std \
  -serial file:/tmp/exoos-serial.log \
  -no-reboot \
  -no-shutdown \
  -monitor none \
  -display none \
  -d int,cpu_reset -D /tmp/exoos-qemu-int.log \
  -debugcon file:/tmp/exoos-e9.log \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -qmp unix:/tmp/exoos-qmp.sock,server=on,wait=off \
  -cdrom exo-os.iso \
  >/tmp/exoos-qemu-stdout.log 2>&1 </dev/null &
echo $! >/tmp/exoos-qemu.pid
for i in $(seq 1 100); do
  [ -S /tmp/exoos-qmp.sock ] && break
  sleep 0.2
done
if [ ! -S /tmp/exoos-qmp.sock ]; then
  cat /tmp/exoos-qemu-stdout.log
  exit 1
fi
sleep 50
OUT_PPM="$OUT_PPM" python3 - <<"PY"
import json, socket
import os
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('/tmp/exoos-qmp.sock')
_ = sock.recv(65536)
sock.sendall((json.dumps({'execute':'qmp_capabilities'})+'\r\n').encode())
_ = sock.recv(65536)
cmd = f"screendump {os.environ['OUT_PPM']}"
sock.sendall((json.dumps({'execute':'human-monitor-command','arguments':{'command-line':cmd}})+'\r\n').encode())
print(sock.recv(65536).decode(errors='ignore'))
sock.close()
PY
OUT_PPM="$OUT_PPM" OUT_PNG="$OUT_PNG" python3 - <<"PY"
import struct, zlib
import os
from pathlib import Path
src = Path(os.environ['OUT_PPM'])
dst = Path(os.environ['OUT_PNG'])
data = src.read_bytes()
idx = 2
tokens = []
while len(tokens) < 3:
    while data[idx:idx+1] in (b' ', b'\n', b'\r', b'\t'):
        idx += 1
    if data[idx:idx+1] == b'#':
        while data[idx:idx+1] != b'\n':
            idx += 1
        continue
    start = idx
    while data[idx:idx+1] not in (b' ', b'\n', b'\r', b'\t'):
        idx += 1
    tokens.append(data[start:idx])
while data[idx:idx+1] in (b' ', b'\n', b'\r', b'\t'):
    idx += 1
w, h, maxv = map(int, tokens)
assert maxv == 255
rgb = data[idx:]
scanlines = b''.join(b'\x00' + rgb[y*w*3:(y+1)*w*3] for y in range(h))
def chunk(tag, payload):
    return struct.pack('!I', len(payload)) + tag + payload + struct.pack('!I', zlib.crc32(tag + payload) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n'
png += chunk(b'IHDR', struct.pack('!IIBBBBB', w, h, 8, 2, 0, 0, 0))
png += chunk(b'IDAT', zlib.compress(scanlines, 9))
png += chunk(b'IEND', b'')
dst.write_bytes(png)
print(f'{w}x{h}')
PY
printf "PID="; cat /tmp/exoos-qemu.pid
printf "\nE9_HEX="
xxd -p /tmp/exoos-e9.log | tr -d "\n"
printf "\nSTATUS="
ps -p $(cat /tmp/exoos-qemu.pid) -o pid=,comm=,stat=,etime=
kill $(cat /tmp/exoos-qemu.pid) 2>/dev/null || true
