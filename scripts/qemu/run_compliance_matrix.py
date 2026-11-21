#!/usr/bin/env python3
"""
Automated QEMU Compliance Matrix Runner for Exo-OS (T0 -> T6).
- Verifies global rustup sysroot was never mutated (T0).
- Clones target/qemu/exofs-root.img to an ephemeral /tmp image (never mutates persistent disk).
- Boots exo-os.iso in QEMU with isa-debug-exit (0xf4 -> exit code 33 on PASS, 35 on FAIL).
- Runs `exo doctor`, `exo wayland` (wayland-rs 0.31.14), and `/bin/exo-compliance-runner --exit-qemu`.
- Parses all `[EXO_TEST]` markers and verifies 100% PASS across T0..T6 + QEMU exit code 33.
"""

from __future__ import annotations

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


def check_rustup_sysroot_purity() -> tuple[bool, str]:
    try:
        out = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
    except Exception as exc:
        return False, f"rustc --print sysroot failed: {exc}"
    pal_exo = Path(out) / "lib" / "rustlib" / "src" / "rust" / "library" / "std" / "src" / "sys" / "pal" / "exo_os"
    if pal_exo.exists():
        return False, f"global rustup sysroot was mutated: {pal_exo} exists"
    staged = REPO_ROOT / "target" / "exo-rust-src" / "library" / "std" / "src" / "sys" / "pal" / "exo_os"
    if not staged.exists():
        return False, f"staged exo-rust-src missing at {staged}"
    return True, f"sysroot clean ({out}), staged tree active"


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


def main() -> int:
    os.chdir(REPO_ROOT)
    base_disk = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO_ROOT / "target" / "qemu" / "exofs-root.img"
    iso_path = REPO_ROOT / "exo-os.iso"

    if not base_disk.exists():
        print(f"[ERROR] Base disk image not found: {base_disk}", file=sys.stderr)
        return 1
    if not iso_path.exists():
        print(f"[ERROR] ISO image not found: {iso_path}", file=sys.stderr)
        return 1

    t0_ok, t0_detail = check_rustup_sysroot_purity()
    pid = os.getpid()
    ephemeral_disk = Path(f"/tmp/exoos-compliance-disk-{pid}.img")
    qmp_path = Path(f"/tmp/exoos-compliance-qmp-{pid}.sock")
    serial_log = Path(f"/tmp/exoos-compliance-serial-{pid}.log")
    e9_log = Path(f"/tmp/exoos-compliance-e9-{pid}.log")
    qemu_out = Path(f"/tmp/exoos-compliance-qemu-{pid}.log")

    shutil.copyfile(base_disk, ephemeral_disk)
    proc: subprocess.Popen[bytes] | None = None

    try:
        qemu_cmd = [
            "qemu-system-x86_64",
            "-machine", "q35",
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
            "-netdev", "user,id=exoe1000",
            "-device", "e1000,netdev=exoe1000,mac=02:45:58:4f:00:02",
            "-cdrom", str(iso_path),
        ]
        with qemu_out.open("wb") as out_f:
            proc = subprocess.Popen(qemu_cmd, stdout=out_f, stderr=subprocess.STDOUT)

        for _ in range(100):
            if qmp_path.exists():
                break
            time.sleep(0.2)
        if not qmp_path.exists():
            print("[ERROR] QMP socket did not appear", file=sys.stderr)
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
                {
                    "keys": [{"type": "qcode", "data": k} for k in keys],
                    "hold-time": 45,
                },
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
                        raise TimeoutError(f"Key echo missing for {ch!r} while sending {text!r}")
                time.sleep(0.25)

        print("[QEMU] Waiting for Exo-OS userspace console ready...")
        wait_for_e9(b"Exo-OS userspace console ready", 600)
        data = wait_for_e9(b"exosh:/", 120)
        seen_prompts = prompt_count(data)
        wait_for_prompt_count(seen_prompts, 120)

        # 1. Run `exo doctor` in exosh
        print("[QEMU] Running `exo doctor`...")
        doc_start = len(e9_bytes())
        send_text("exo doctor\n")
        seen_prompts += 1
        data = wait_for_prompt_count(seen_prompts, 120)
        doc_out = data[doc_start:].decode("utf-8", "replace")
        doc_pass = doc_out.count("[PASS]")
        doc_fail = doc_out.count("[FAIL]")
        doctor_ok = doc_pass >= 8 and doc_fail == 0

        # 2. Run `exo wayland` (wayland-rs 0.31.14 live client -> fb_server)
        print("[QEMU] Running `exo wayland`...")
        wl_start = len(e9_bytes())
        send_text("exo wayland\n")
        seen_prompts += 1
        data = wait_for_prompt_count(seen_prompts, 120)
        wl_out = data[wl_start:].decode("utf-8", "replace")
        wayland_ok = (
            re.search(r"Rust std PAL\s*:\s*LIVE OK", wl_out) is not None
            and re.search(r"wayland-rs std\s*:\s*LIVE OK", wl_out) is not None
            and "buffer_released=1" in wl_out
        )

        # 3. Run `/bin/exo-compliance-runner --exit-qemu`
        print("[QEMU] Running `/bin/exo-compliance-runner --exit-qemu`...", flush=True)
        runner_start_pos = len(e9_bytes())
        send_text("/bin/exo-compliance-runner --exit-qemu\n")

        exit_deadline = time.time() + 180
        qemu_rc: int | None = None
        printed_lines: set[str] = set()
        while time.time() < exit_deadline:
            chunk = e9_bytes()[runner_start_pos:].decode("utf-8", "replace")
            for line in chunk.splitlines():
                if "[EXO_TEST]" in line and line not in printed_lines:
                    printed_lines.add(line)
                    print(f"  {line}", flush=True)
            qemu_rc = proc.poll()
            if qemu_rc is not None:
                break
            time.sleep(0.5)

        if qemu_rc is None:
            tail = e9_bytes()[runner_start_pos:].decode("utf-8", "replace")
            print("\n[TIMEOUT DIAGNOSTIC] E9 output after runner start:\n" + tail, file=sys.stderr, flush=True)
            if serial_log.exists():
                stail = serial_log.read_bytes()[-4096:].decode("utf-8", "replace")
                print("\n[TIMEOUT DIAGNOSTIC] Serial log tail:\n" + stail, file=sys.stderr, flush=True)
            raise TimeoutError("QEMU did not exit within 180s after launching exo-compliance-runner")

        e9_text = e9_bytes().decode("utf-8", "replace")
        case_re = re.compile(
            r"\[EXO_TEST\] CASE=(\S+) TIER=(\S+) RESULT=(PASS|FAIL) DURATION_MS=(\d+)(?: STATUS=\"([^\"]*)\")?(?: DETAIL=\"([^\"]*)\")?"
        )
        summary_re = re.compile(r"\[EXO_TEST\] SUMMARY PASS=(\d+) FAIL=(\d+) TOTAL=(\d+)")

        cases = case_re.findall(e9_text)
        summary_m = summary_re.search(e9_text)

        print("\n" + "=" * 78)
        print("EXO-OS STD & POSIX COMPLIANCE MATRIX (QEMU LIVE VERIFICATION)")
        print("=" * 78)
        print(f"[{'PASS' if t0_ok else 'FAIL'}] T0  host_rustup_sysroot_clean        ({t0_detail})")
        print(f"[{'PASS' if doctor_ok else 'FAIL'}] T0  exosh_exo_doctor                 (pass={doc_pass}, fail={doc_fail})")
        print(f"[{'PASS' if wayland_ok else 'FAIL'}] T4  exosh_wayland_rs_client          (wayland-rs 0.31.14 wl_shm roundtrip)")

        all_cases_ok = len(cases) >= 13
        for name, tier, result, ms, status_s, detail_s in cases:
            if result != "PASS":
                all_cases_ok = False
            extra = f" [{status_s}] {detail_s}" if detail_s or status_s else ""
            print(f"[{result}] {tier:<3} {name:<32} ({ms} ms){extra}")

        print("-" * 78)
        if summary_m:
            s_pass, s_fail, s_total = map(int, summary_m.groups())
            print(f"Runner Summary : PASS={s_pass} FAIL={s_fail} TOTAL={s_total}")
            if s_fail != 0 or s_pass != s_total:
                all_cases_ok = False
        else:
            print("Runner Summary : MISSING [EXO_TEST] SUMMARY marker!")
            all_cases_ok = False

        print(f"QEMU Exit Code : {qemu_rc} (expected 33 via isa-debug-exit 0xf4)")
        print("=" * 78)

        if not (t0_ok and doctor_ok and wayland_ok and all_cases_ok and qemu_rc == 33):
            return 1
        return 0

    finally:
        if proc is not None and proc.poll() is None:
            proc.kill()
            try:
                proc.wait(timeout=5)
            except Exception:
                pass
        for f in (ephemeral_disk, qmp_path, serial_log, e9_log, qemu_out):
            try:
                if f.exists():
                    f.unlink()
            except Exception:
                pass


if __name__ == "__main__":
    sys.exit(main())
