set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
export PATH="$HOME/.cargo/bin:$PATH"
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
cd "$REPO_ROOT"
pkill -f "qemu-system-x86_64.*exo-os.iso" 2>/dev/null || true
rm -f /tmp/exoos-qmp.sock /tmp/exoos-qemu.pid /tmp/exoos-serial.log /tmp/exoos-e9.log /tmp/exoos-qemu-int.log /tmp/exoos-screen.ppm /tmp/exoos-screen.png /tmp/exoos-qemu-stdout.log /tmp/exoos-make.log
if ! make iso >/tmp/exoos-make.log 2>&1; then
  cat /tmp/exoos-make.log
  exit 1
fi
qemu-system-x86_64 \
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
  >/tmp/exoos-qemu-stdout.log 2>&1 &
echo $! >/tmp/exoos-qemu.pid
for i in $(seq 1 100); do
  [ -S /tmp/exoos-qmp.sock ] && break
  sleep 0.2
done
if [ ! -S /tmp/exoos-qmp.sock ]; then
  cat /tmp/exoos-qemu-stdout.log
  exit 1
fi
sleep 8
python3 - <<"PY"
import json, socket
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('/tmp/exoos-qmp.sock')
_ = sock.recv(65536)
sock.sendall((json.dumps({'execute':'qmp_capabilities'})+'\r\n').encode())
_ = sock.recv(65536)
sock.sendall((json.dumps({'execute':'human-monitor-command','arguments':{'command-line':'screendump /tmp/exoos-screen.ppm'}})+'\r\n').encode())
resp = sock.recv(65536)
print(resp.decode(errors='ignore'))
sock.close()
PY
printf "E9_HEX="
xxd -p /tmp/exoos-e9.log | tr -d "\n"
printf "\nPID="; cat /tmp/exoos-qemu.pid
printf "\n"
