#!/usr/bin/env bash
set -euo pipefail

DISK_IMAGE=${1:-target/qemu/exofs-root.img}
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
cd "$REPO_ROOT"

QMP=${EXOOS_SHELL_QMP:-/tmp/exoos-shell-qmp.sock}
SERIAL=${EXOOS_SHELL_SERIAL:-/tmp/exoos-shell-serial.log}
E9=${EXOOS_SHELL_E9:-/tmp/exoos-shell-e9.log}
INTLOG=${EXOOS_SHELL_INTLOG:-/tmp/exoos-shell-int.log}
STDOUT=${EXOOS_SHELL_STDOUT:-/tmp/exoos-shell-qemu.log}
SCREEN=${EXOOS_SHELL_SCREEN:-/tmp/exoos-shell-screen.ppm}
PIDFILE=${EXOOS_SHELL_PIDFILE:-/tmp/exoos-shell-qemu.pid}
QEMU_TRACE_ARGS=()

if [[ "${EXOOS_SHELL_INT_TRACE:-0}" != "0" ]]; then
  QEMU_TRACE_ARGS=(-d int,cpu_reset -D "$INTLOG")
fi

mkdir -p "$(dirname "$QMP")" "$(dirname "$SERIAL")" "$(dirname "$E9")" \
  "$(dirname "$INTLOG")" "$(dirname "$STDOUT")" "$(dirname "$SCREEN")" \
  "$(dirname "$PIDFILE")"

pkill -f "qemu-system-x86_64.*exoos-shell-qmp" 2>/dev/null || true
rm -f "$QMP" "$SERIAL" "$E9" "$INTLOG" "$STDOUT" "$SCREEN" "$PIDFILE"

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
  "${QEMU_TRACE_ARGS[@]}" \
  -debugcon file:"$E9" \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -qmp unix:"$QMP",server=on,wait=off \
  -drive if=none,file="$DISK_IMAGE",format=raw,id=exofs0,cache=writeback \
  -device virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16 \
  -netdev user,id=exovirtio \
  -device virtio-net-pci-non-transitional,netdev=exovirtio,mac=02:45:58:4f:00:01 \
  -netdev user,id=exoe1000 \
  -device e1000,netdev=exoe1000,mac=02:45:58:4f:00:02 \
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

python3 - "$QMP" "$SCREEN" "$E9" <<'PY'
import json
import os
from pathlib import Path
import socket
import sys
import time

qmp = sys.argv[1]
screen = sys.argv[2]
e9 = Path(sys.argv[3])
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

def qmp_command(command, arguments=None):
    request = {"execute": command}
    if arguments is not None:
        request["arguments"] = arguments
    sock.sendall((json.dumps(request) + "\r\n").encode())
    return sock.recv(65536)

def e9_bytes():
    try:
        return e9.read_bytes()
    except FileNotFoundError:
        return b""

def wait_for_e9(needle, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        data = e9_bytes()
        if needle in data:
            return data
        time.sleep(1.0)
    raise TimeoutError(needle.decode("ascii", "replace"))

def prompt_count(data):
    return data.count(b"\nexosh:") + data.count(b"\rexosh:") + data.count(b"\x0cexosh:")

def input_prompt_ready(data):
    prompt_start = max(
        data.rfind(b"\nexosh:"),
        data.rfind(b"\rexosh:"),
        data.rfind(b"\x0cexosh:"),
    )
    return prompt_start >= 0 and b"\x1b[7m" in data[prompt_start:]

def wait_for_prompt_count(target, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        data = e9_bytes()
        if prompt_count(data) >= target and input_prompt_ready(data):
            return data
        time.sleep(0.25)
    raise TimeoutError(f"shell input prompt count stayed below {target}")

KEYS = {
    "\n": ["ret"],
    " ": ["spc"],
    "/": ["slash"],
    "=": ["equal"],
    ">": ["shift", "dot"],
    ".": ["dot"],
    "-": ["minus"],
    "_": ["shift", "minus"],
    "*": ["shift", "8"],
}
key_hold_ms = int(os.environ.get("EXOOS_SHELL_KEY_HOLD_MS", "20"))
key_delay = float(os.environ.get("EXOOS_SHELL_KEY_DELAY", "0.12"))
key_echo_timeout = float(os.environ.get("EXOOS_SHELL_KEY_ECHO_TIMEOUT", "2.0"))

def send_key(keys):
    if isinstance(keys, str):
        keys = [keys]
    qmp_command("send-key", {
        "keys": [{"type": "qcode", "data": key} for key in keys],
        "hold-time": key_hold_ms,
    })

def wait_for_key_echo(ch, start, timeout):
    needle = ch.encode("ascii")
    deadline = time.time() + timeout
    while time.time() < deadline:
        data = e9_bytes()
        if needle in data[start:]:
            return
        time.sleep(0.01)
    raise TimeoutError(f"shell key echo did not include {needle!r}")

def send_text(text):
    for ch in text:
        echo_start = len(e9_bytes())
        send_key(KEYS.get(ch, ch))
        if ch != "\n":
            wait_for_key_echo(ch, echo_start, key_echo_timeout)
        time.sleep(key_delay)

ready_timeout = int(os.environ.get("EXOOS_SHELL_READY_TIMEOUT", "600"))
wait_for_e9(b"Exo-OS userspace console ready", ready_timeout)
prompt_timeout = int(os.environ.get("EXOOS_SHELL_PROMPT_TIMEOUT", "120"))
data = wait_for_e9(b"exosh:/", prompt_timeout)
seen_prompts = prompt_count(data)
data = wait_for_prompt_count(seen_prompts, prompt_timeout)
command_timeout = int(os.environ.get("EXOOS_SHELL_COMMAND_TIMEOUT", "120"))

commands = [
    "pwd\n",
    "mkdir /tmp\n",
    "mkdir /tmp/t\n",
    "mkdir /tmp/d\n",
    "mkdir /tmp/glob\n",
    "mkdir /tmp/hidden\n",
    "touch /tmp/t/a\n",
    "touch /tmp/t/.h\n",
    "echo hi > /tmp/t/a\n",
    "cat /tmp/t/a\n",
    "cp /tmp/t/a /tmp/d\n",
    "cat /tmp/d/a\n",
    "mv /tmp/d/a /tmp/t/b\n",
    "echo m > /tmp/d/m\n",
    "mv /tmp/d/m /tmp/t\n",
    "cat /tmp/t/m\n",
    "ls -lah /tmp/t\n",
    "ls -lah /tmp\n",
    "tree /tmp\n",
    "time echo bench\n",
    "dd if=/dev/zero of=/tmp/bench bs=1k count=2\n",
    "dd if=/tmp/bench of=/dev/null bs=1k\n",
    "rm /tmp/bench\n",
    "cd /tmp/t\n",
    "pwd\n",
    "cd /\n",
    "touch /tmp/glob/x\n",
    "touch /tmp/glob/y\n",
    "touch /tmp/hidden/.h\n",
    "cd /tmp/glob\n",
    "rm -f *\n",
    "cd /\n",
    "rm -rf /tmp/hidden\n",
    "history\n",
    "rm -rf /tmp/d\n",
    "rm /tmp/t/a\n",
    "rm /tmp/t/.hidden\n",
    "rm /tmp/t/b\n",
    "rm /tmp/t/m\n",
    "rmdir /tmp/t\n",
    "rmdir /tmp/glob\n",
    "rmdir /tmp\n",
    "clear\n",
    "top\n",
]
for text in commands:
    send_text(text)
    seen_prompts += 1
    wait_for_prompt_count(seen_prompts, command_timeout)
    if text == "clear\n":
        send_key(["ctrl", "l"])
        time.sleep(0.1)

send_text("echo first\n")
seen_prompts += 1
wait_for_prompt_count(seen_prompts, command_timeout)

send_key("up")
time.sleep(0.2)
send_key("ret")
seen_prompts += 1
wait_for_prompt_count(seen_prompts, command_timeout)

send_text("echo ab")
send_key("left")
time.sleep(0.1)
send_key("right")
time.sleep(0.1)
send_text("c\n")
seen_prompts += 1
wait_for_prompt_count(seen_prompts, command_timeout)

send_key("up")
time.sleep(0.1)
send_key("down")
time.sleep(0.1)
send_text("echo arrows\n")
seen_prompts += 1
wait_for_prompt_count(seen_prompts, 45)

if b"hi\n" not in e9_bytes():
    raise RuntimeError("cat /tmp/t/a did not echo expected data")

if b"m\n" not in e9_bytes():
    raise RuntimeError("mv into directory did not preserve readable file")

if b"/tmp/t\n" not in e9_bytes():
    raise RuntimeError("cd /tmp/t ; pwd did not report expected cwd")

if b"drwx" not in e9_bytes() and b"-rw" not in e9_bytes():
    raise RuntimeError("ls -lah did not print long file metadata")

if b".h" not in e9_bytes():
    raise RuntimeError("ls -lah did not include hidden files")

if b"\x1b[1;34m" not in e9_bytes():
    raise RuntimeError("ls did not emit directory color")

if e9_bytes().count(b"first\n") < 2:
    raise RuntimeError("arrow up did not recall and execute the previous command")

if b"abc\n" not in e9_bytes():
    raise RuntimeError("left/right cursor movement did not preserve insertion position")

if b"arrows\n" not in e9_bytes():
    raise RuntimeError("arrow up/down line editing did not leave a clean command")

if b"\x1b[7m" not in e9_bytes():
    raise RuntimeError("line editor did not render a visible cursor")

if b"bench\n" not in e9_bytes() or b"real " not in e9_bytes():
    raise RuntimeError("time builtin did not run command and print elapsed time")

if e9_bytes().count(b"bytes copied in") < 2:
    raise RuntimeError("dd builtin did not complete write/read benchmark paths")

if (
    b"PID  NAME              STATE" not in e9_bytes()
    or b"12   exo_shield" not in e9_bytes()
    or b"13   exosh" not in e9_bytes()
):
    raise RuntimeError("top did not report expected service PID/name mapping")

hmp("screendump " + screen)
sock.close()
PY

echo "SERIAL=$SERIAL"
echo "E9=$E9"
echo "SCREEN=$SCREEN"
