#!/usr/bin/env python3
"""
Automated ExoPhoenix Indisputable Qualification Runner (F0 -> F9, P5 -> P8).

Supports:
  - Palier 1: QEMU TCG multi-core (`-smp 4`, `-cpu qemu64,+sse2,+avx,+xsave,+rdtscp`)
  - Palier 2: QEMU KVM + Intel IOMMU (`VT-d`, `-machine q35,accel=kvm,kernel-irqchip=split`,
              `-device intel-iommu,intremap=on,caching-mode=on,device-iotlb=on`, `-smp 4`)
  - Suites: faults (F0+F1), smp-cold (F2+F3), gi05 (F4+F5+F6+F7+F8+F9), all (F0..F9)
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


def run_f0_pure_hygiene(pure_root: Path, pure_kernel: Path) -> bool:
    cmd = [
        sys.executable,
        str(REPO_ROOT / "scripts" / "qemu" / "run_security_audit.py"),
        "--s0-only",
        "--pure-root",
        str(pure_root),
        "--pure-kernel",
        str(pure_kernel),
    ]
    res = subprocess.run(cmd, cwd=REPO_ROOT)
    return res.returncode == 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--disk", default="target/qemu/exofs-root.img")
    ap.add_argument("--iso", default="exo-os-fault-test.iso")
    ap.add_argument("--suite", default="all", choices=["faults", "smp-cold", "gi05", "all", "kvm"])
    ap.add_argument("--accel", default="auto", choices=["auto", "tcg", "kvm"])
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--skip-f0", action="store_true")
    ap.add_argument("--pure-root", default="target/rootfs-pure-staging")
    ap.add_argument("--pure-kernel", default="target/exophoenix/pure-release-kernel.elf")
    ap.add_argument("--log-out", default="")
    args = ap.parse_args()

    os.chdir(REPO_ROOT)

    if not args.skip_f0 and args.suite in ("faults", "all", "kvm"):
        print("[EXO_QUAL] Running F0 (S0 pure-profile binary hygiene)...", flush=True)
        pure_kernel = Path(args.pure_kernel)
        if not pure_kernel.exists():
            pure_kernel = Path("target/x86_64-unknown-none/release/exo-os-kernel")
        if not run_f0_pure_hygiene(Path(args.pure_root), pure_kernel):
            print("[EXO_QUAL] F0 FAIL: pure release kernel/rootfs hygiene violated!", file=sys.stderr)
            return 1
        print("[EXO_QUAL] F0 PASS: 0 phoenix_inject_* / 0 fault_inject / 0 out 0xf4 / 0 libc.so", flush=True)

    base_disk = Path(args.disk)
    iso_path = Path(args.iso)
    if not base_disk.exists() or not iso_path.exists():
        print(f"[ERROR] Missing disk ({base_disk}) or ISO ({iso_path})", file=sys.stderr)
        return 1

    has_kvm = os.path.exists("/dev/kvm") and os.access("/dev/kvm", os.R_OK | os.W_OK)
    if args.accel == "kvm":
        if not has_kvm:
            print(
                "[EXO_QUAL] BLOCKED: --accel kvm requested but /dev/kvm is not read/write accessible "
                "for current user (check `id` and `ls -la /dev/kvm`; add user to `kvm` group).",
                file=sys.stderr,
            )
            return 2
        use_kvm = True
    elif args.accel == "tcg":
        use_kvm = False
    else:
        use_kvm = has_kvm

    pid = os.getpid()
    ephemeral_disk = Path(f"/tmp/exoos-qual-disk-{pid}.img")
    qmp_path = Path(f"/tmp/exoos-qual-qmp-{pid}.sock")
    serial_log = Path(f"/tmp/exoos-qual-serial-{pid}.log")
    e9_log = Path(f"/tmp/exoos-qual-e9-{pid}.log")
    qemu_out = Path(f"/tmp/exoos-qual-qemu-{pid}.log")

    shutil.copyfile(base_disk, ephemeral_disk)
    proc: subprocess.Popen[bytes] | None = None

    try:
        if use_kvm:
            qemu_cmd = [
                "qemu-system-x86_64",
                "-machine", "q35,accel=kvm,kernel-irqchip=split",
                "-device", "intel-iommu,intremap=on,caching-mode=on,device-iotlb=on",
                "-device", "edu",
                "-cpu", "host",
                "-smp", str(args.smp),
                "-m", args.mem,
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
        else:
            qemu_cmd = [
                "qemu-system-x86_64",
                "-machine", "q35",
                "-device", "intel-iommu,intremap=off,caching-mode=on",
                "-device", "edu",
                "-smp", str(args.smp),
                "-m", args.mem,
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

        accel_label = "KVM+VT-d" if use_kvm else "TCG+VT-d"
        print(
            f"[EXO_QUAL] Launching QEMU ({accel_label}, -smp {args.smp}, suite={args.suite}, iso={iso_path})...",
            flush=True,
        )
        with qemu_out.open("wb") as out_f:
            proc = subprocess.Popen(qemu_cmd, stdout=out_f, stderr=subprocess.STDOUT)

        for _ in range(100):
            if qmp_path.exists():
                break
            if proc.poll() is not None:
                print(f"[EXO_QUAL] QEMU exited early rc={proc.returncode}:\n{qemu_out.read_text(errors='replace')}", file=sys.stderr)
                return 1
            time.sleep(0.2)
        if not qmp_path.exists():
            print("[EXO_QUAL] QMP socket did not appear", file=sys.stderr)
            return 1

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
                if proc and proc.poll() is not None:
                    raise RuntimeError(f"QEMU exited early rc={proc.returncode} while waiting for {needle!r}")
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

        print("[EXO_QUAL] Waiting for Exo-OS userspace console ready...", flush=True)
        wait_for_e9(b"Exo-OS userspace console ready", 600)
        data = wait_for_e9(b"exosh:/", 180)
        seen_prompts = prompt_count(data)
        wait_for_prompt_count(seen_prompts, 180)

        cmd_line = f"/bin/exo-compliance-runner --phoenix-qual --suite {args.suite} --exit-qemu\n"
        print(f"[EXO_QUAL] Running `{cmd_line.strip()}`...", flush=True)
        start_pos = len(e9_bytes())
        send_text(cmd_line)

        exit_deadline = time.time() + args.timeout
        qemu_rc: int | None = None
        printed: set[str] = set()
        qual_lines: list[str] = []
        while time.time() < exit_deadline:
            chunk = e9_bytes()[start_pos:].decode("utf-8", "replace")
            for line in chunk.splitlines():
                if (
                    "[EXO_QUAL]" in line
                    or "[EXO_PHOENIX_SLA]" in line
                    or "[ExoPhoenix]" in line
                ) and line not in printed:
                    printed.add(line)
                    qual_lines.append(line)
                    print(f"  {line}", flush=True)
            qemu_rc = proc.poll()
            if qemu_rc is not None:
                break
            time.sleep(0.5)

        final_chunk = e9_bytes()[start_pos:].decode("utf-8", "replace")
        for line in final_chunk.splitlines():
            if (
                "[EXO_QUAL]" in line
                or "[EXO_PHOENIX_SLA]" in line
                or "[ExoPhoenix]" in line
            ) and line not in printed:
                printed.add(line)
                qual_lines.append(line)
                print(f"  {line}", flush=True)

        if args.log_out:
            Path(args.log_out).parent.mkdir(parents=True, exist_ok=True)
            Path(args.log_out).write_text(e9_bytes().decode("utf-8", "replace"), encoding="utf-8")

        summary_ok = any(
            re.search(r"\[EXO_QUAL\]\s+SUMMARY\s+PASS=\d+\s+FAIL=0\s+TOTAL=[1-9]\d*", ln)
            for ln in qual_lines
        )
        print(f"[EXO_QUAL] QEMU exit code: {qemu_rc} (expected 33 on PASS)", flush=True)
        if qemu_rc == 33 and summary_ok:
            print(f"[EXO_QUAL] SUITE={args.suite} ({accel_label} -smp {args.smp}) ALL PASS", flush=True)
            return 0
        print(f"[EXO_QUAL] SUITE={args.suite} FAILED (rc={qemu_rc})", file=sys.stderr)
        tail = e9_bytes()[-4000:].decode("utf-8", "replace")
        print("--- E9 TAIL ---\n" + tail, file=sys.stderr)
        return 1
    except Exception as exc:
        print(f"[EXO_QUAL] EXCEPTION: {exc}", file=sys.stderr)
        try:
            raw = e9_log.read_bytes()
            if args.log_out:
                Path(args.log_out).parent.mkdir(parents=True, exist_ok=True)
                Path(args.log_out).write_bytes(raw)
            print("--- E9 TAIL (on exception) ---\n" + raw[-4000:].decode("utf-8", "replace"), file=sys.stderr)
        except OSError:
            pass
        return 1
    finally:
        if proc and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        for p in (ephemeral_disk, qmp_path, serial_log, e9_log, qemu_out):
            try:
                if p.exists():
                    p.unlink()
            except OSError:
                pass


if __name__ == "__main__":
    raise SystemExit(main())
