#!/usr/bin/env python3
"""
Automated Security Audit Matrix Runner for Exo-OS (S0 -> S6).
Verifies:
  - S0: Pure-profile build hygiene (0 libc.so/libc.a, 0 mimalloc symbols, 0 port 0xf4 refs in pure kernel)
  - S1..S6: Live QEMU adversarial suite via `/bin/exo-compliance-runner --security-audit --exit-qemu`
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import time

REPO_ROOT = Path(__file__).resolve().parents[2]

KEYS: dict[str, list[str]] = {
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


def audit_s0_pure_hygiene(pure_root: Path, pure_kernel: Path) -> tuple[bool, list[str]]:
    issues: list[str] = []

    if not pure_root.exists():
        issues.append(f"pure rootfs staging missing: {pure_root}")
    else:
        forbidden_files = [
            "lib/libc.so",
            "lib/libc.a",
            "lib/crt1.o",
            "lib/Scrt1.o",
            "lib/rcrt1.o",
            "lib/ld-exo.so",
        ]
        for rel in forbidden_files:
            if (pure_root / rel).exists():
                issues.append(f"forbidden C/compat artifact found in pure rootfs: /{rel}")

        # Check all staged ELF binaries for stray mimalloc C symbols
        for bin_dir in (pure_root / "sbin", pure_root / "bin"):
            if not bin_dir.exists():
                continue
            for elf in sorted(bin_dir.iterdir()):
                if not elf.is_file():
                    continue
                try:
                    out = subprocess.check_output(
                        ["nm", "-a", str(elf)],
                        stderr=subprocess.DEVNULL,
                        text=True,
                    )
                    if re.search(r"\bmi_(malloc|free|calloc|realloc|zalloc)\b", out):
                        issues.append(f"mimalloc symbol found in pure binary: {elf.name}")
                except Exception:
                    # Stripped binaries have no symtab; check strings as backup
                    raw = elf.read_bytes()
                    if b"mi_malloc" in raw or b"mimalloc" in raw:
                        issues.append(f"mimalloc string found in pure binary: {elf.name}")

    if not pure_kernel.exists():
        issues.append(f"pure release kernel missing: {pure_kernel}")
    else:
        try:
            nm_out = subprocess.check_output(["nm", str(pure_kernel)], stderr=subprocess.DEVNULL, text=True)
            for forbidden_sym in ("platform_debug_exit", "qemu_debug_exit", "mi_malloc", "mi_free"):
                if forbidden_sym in nm_out:
                    issues.append(f"forbidden symbol '{forbidden_sym}' present in pure release kernel")
        except Exception as exc:
            issues.append(f"nm failed on pure kernel: {exc}")

        try:
            obj_out = subprocess.check_output(
                ["objdump", "-d", str(pure_kernel)],
                stderr=subprocess.DEVNULL,
                text=True,
            )
            if re.search(r"\bout\s+%eax,\$0xf4\b|\bout\s+\$0xf4,%eax\b", obj_out):
                issues.append("port 0xf4 isa-debug-exit instruction found in pure release kernel")
        except Exception as exc:
            issues.append(f"objdump failed on pure kernel: {exc}")

    return len(issues) == 0, issues


def prompt_count(data: bytes) -> int:
    return (
        data.count(b"\n\rexosh:")
        + data.count(b"\nexosh:")
        + data.count(b"\x0c\rexosh:")
        + data.count(b"\x0cexosh:")
    )


def input_prompt_ready(data: bytes) -> bool:
    prompt_start = max(
        data.rfind(b"\n\rexosh:"),
        data.rfind(b"\nexosh:"),
        data.rfind(b"\x0c\rexosh:"),
        data.rfind(b"\x0cexosh:"),
    )
    return prompt_start >= 0 and b"\x1b[7m" in data[prompt_start:]


def run_live_security_audit(base_disk: Path, iso_path: Path, smp: int = 1) -> tuple[bool, list[str], int | None]:
    pid = os.getpid()
    ephemeral_disk = Path(f"/tmp/exoos-sec-disk-{pid}.img")
    qmp_path = Path(f"/tmp/exoos-sec-qmp-{pid}.sock")
    serial_log = Path(f"/tmp/exoos-sec-serial-{pid}.log")
    e9_log = Path(f"/tmp/exoos-sec-e9-{pid}.log")
    qemu_out = Path(f"/tmp/exoos-sec-qemu-{pid}.log")

    shutil.copyfile(base_disk, ephemeral_disk)
    proc: subprocess.Popen[bytes] | None = None
    sec_lines: list[str] = []

    try:
        qemu_cmd = [
            "qemu-system-x86_64",
            "-machine", "q35",
            "-smp", str(smp),
            "-m", "2G",
            "-boot", "d",
            "-vga", "std",
            "-serial", f"file:{serial_log}",
            "-no-reboot",
            "-no-shutdown",
            "-monitor", "none",
            "-display", "none",
            "-debugcon", f"file:{e9_log}",
            "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-qmp", f"unix:{qmp_path},server=on,wait=off",
            "-drive", f"if=none,file={ephemeral_disk},format=raw,id=exofs0,cache=writeback",
            "-device", "virtio-blk-pci,drive=exofs0,disable-modern=on,disable-legacy=off,indirect_desc=off,event_idx=off,queue-size=16",
            "-netdev", "user,id=exovirtio",
            "-device", "virtio-net-pci-non-transitional,netdev=exovirtio,mac=02:45:58:4f:00:01",
            "-cdrom", str(iso_path),
        ]
        with qemu_out.open("wb") as out_f:
            proc = subprocess.Popen(qemu_cmd, stdout=out_f, stderr=subprocess.STDOUT)

        for _ in range(100):
            if qmp_path.exists():
                break
            time.sleep(0.2)
        if not qmp_path.exists():
            return False, ["QMP socket did not appear"], None

        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(str(qmp_path))
        sock.recv(65536)
        sock.sendall((json.dumps({"execute": "qmp_capabilities"}) + "\r\n").encode())
        sock.recv(65536)

        def qmp_command(command: str, arguments: dict | None = None) -> bytes:
            req: dict = {"execute": command}
            if arguments is not None:
                req["arguments"] = arguments
            sock.sendall((json.dumps(req) + "\r\n").encode())
            return sock.recv(65536)

        def e9_bytes() -> bytes:
            try:
                return e9_log.read_bytes()
            except FileNotFoundError:
                return b""

        def wait_for_e9(needle: bytes, timeout_s: float) -> bytes:
            deadline = time.time() + timeout_s
            while time.time() < deadline:
                data = e9_bytes()
                if needle in data:
                    return data
                time.sleep(0.5)
            raise TimeoutError(f"Timed out waiting for {needle!r} in E9 log")

        def wait_for_prompt_count(target: int, timeout_s: float) -> bytes:
            deadline = time.time() + timeout_s
            while time.time() < deadline:
                data = e9_bytes()
                if prompt_count(data) >= target and input_prompt_ready(data):
                    return data
                time.sleep(0.25)
            raise TimeoutError(f"Timed out waiting for prompt count {target}")

        def send_key(keys: list[str] | str) -> None:
            if isinstance(keys, str):
                keys = [keys]
            qmp_command(
                "send-key",
                {"keys": [{"type": "qcode", "data": k} for k in keys], "hold-time": 45},
            )

        def wait_for_key_echo(ch: str, start: int, timeout_s: float = 3.0) -> bool:
            needle = ch.encode("ascii")
            deadline = time.time() + timeout_s
            while time.time() < deadline:
                if needle in e9_bytes()[start:]:
                    return True
                time.sleep(0.01)
            return False

        def send_text(text: str) -> None:
            for ch in text:
                start = len(e9_bytes())
                send_key(KEYS.get(ch, ch))
                if ch != "\n" and not wait_for_key_echo(ch, start, 3.0):
                    start = len(e9_bytes())
                    send_key(KEYS.get(ch, ch))
                    if not wait_for_key_echo(ch, start, 3.0):
                        raise TimeoutError(f"Key echo missing for {ch!r}")
                time.sleep(0.2)

        print("[SEC-QEMU] Waiting for Exo-OS userspace console ready...", flush=True)
        wait_for_e9(b"Exo-OS userspace console ready", 600)
        data = wait_for_e9(b"exosh:/", 120)
        seen_prompts = prompt_count(data)
        wait_for_prompt_count(seen_prompts, 120)

        print("[SEC-QEMU] Running `/bin/exo-compliance-runner --security-audit --exit-qemu`...", flush=True)
        start_pos = len(e9_bytes())
        send_text("/bin/exo-compliance-runner --security-audit --exit-qemu\n")

        exit_deadline = time.time() + 240
        qemu_rc: int | None = None
        printed: set[str] = set()
        while time.time() < exit_deadline:
            chunk = e9_bytes()[start_pos:].decode("utf-8", "replace")
            for line in chunk.splitlines():
                if ("[EXO_SEC]" in line or "[EXO_PHOENIX_SLA]" in line) and line not in printed:
                    printed.add(line)
                    sec_lines.append(line)
                    print(f"  {line}", flush=True)
            qemu_rc = proc.poll()
            if qemu_rc is not None:
                break
            time.sleep(0.5)

        if qemu_rc is None:
            tail = e9_bytes()[start_pos:].decode("utf-8", "replace")
            print("[SEC-QEMU] TIMEOUT tail:\n" + tail[-4096:], file=sys.stderr, flush=True)
            return False, sec_lines, None

        sock.close()
        required = ["S1", "S2", "S3", "S4", "S5", "S6"]
        all_ok = qemu_rc == 33
        for tier in required:
            if not any(f"[EXO_SEC] {tier} PASS" in l for l in sec_lines):
                all_ok = False
        return all_ok, sec_lines, qemu_rc
    finally:
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except Exception:
                proc.kill()
        for p in (ephemeral_disk, qmp_path, serial_log, e9_log, qemu_out):
            try:
                p.unlink(missing_ok=True)
            except Exception:
                pass


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pure-root", default="target/rootfs-pure-staging")
    ap.add_argument("--pure-kernel", default="target/x86_64-unknown-none/release/exo-os-kernel")
    ap.add_argument("--disk", default="target/qemu/exofs-root.img")
    ap.add_argument("--iso", default="exo-os.iso")
    ap.add_argument("--s0-only", action="store_true")
    ap.add_argument("--smp", type=int, default=1)
    args = ap.parse_args()

    os.chdir(REPO_ROOT)
    s0_ok, s0_issues = audit_s0_pure_hygiene(Path(args.pure_root), Path(args.pure_kernel))
    print(f"[EXO_SEC] S0 {'PASS' if s0_ok else 'FAIL'} pure-profile hygiene", flush=True)
    for iss in s0_issues:
        print(f"  [S0 ISSUE] {iss}", file=sys.stderr, flush=True)

    if args.s0_only:
        return 0 if s0_ok else 1

    live_ok, sec_lines, qemu_rc = run_live_security_audit(Path(args.disk), Path(args.iso), smp=args.smp)
    print(
        f"\n[SECURITY AUDIT SUMMARY] S0={'PASS' if s0_ok else 'FAIL'} "
        f"LIVE_S1_S6={'PASS' if live_ok else 'FAIL'} QEMU_RC={qemu_rc}",
        flush=True,
    )
    return 0 if (s0_ok and live_ok) else 1


if __name__ == "__main__":
    sys.exit(main())
