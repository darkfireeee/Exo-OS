#!/usr/bin/env python3
"""
Automated ExoPhoenix 3-Strata O(1) Invariance & SLA Benchmark Runner (Spec §6.2).

Executes two configurations via `SYS_PHOENIX_FORCE (523)` inside QEMU:
  - Cas A: Baseline Strate 1 only (Strate 2 quiescent)
  - Cas B: Full Strate 2 load (active Wayland /run/wayland-0 + loopback TCP + concurrent workers)

Acceptance Criteria:
  1. Absolute SLA (when KVM + --release): S0_TOTAL_US <= 100_000 us, S1_TOTAL_US <= 1_000_000 us
  2. O(1) Invariance (valid even under QEMU TCG):
       |T_SLA(Cas B) - T_SLA(Cas A)| / T_SLA(Cas A) <= 15%
  3. Post-switch integrity: DEGRADED=0, RESULT=PASS, ExoLedger verified, monotonic crypto epoch_id.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import socket
import statistics
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

SLA_RE = re.compile(
    r"\[EXO_PHOENIX_SLA\]\s+CYCLE=(\d+)\s+REASON=(\S+)\s+"
    r"S0_FREEZE_US=(\d+)\s+S0_FORGE_US=(\d+)\s+S0_TOTAL_US=(\d+)\s+"
    r"CRYPTO_US=(\d+)\s+S1_TOTAL_US=(\d+)\s+FULL_S2_US=(\d+)\s+"
    r"DEGRADED=(\d+)\s+RESULT=(\S+)(?:\s+EPOCH=(\d+))?"
)


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


def parse_sla_records(text: str) -> list[dict]:
    out = []
    for m in SLA_RE.finditer(text):
        out.append({
            "cycle": int(m.group(1)),
            "reason": m.group(2),
            "s0_freeze_us": int(m.group(3)),
            "s0_forge_us": int(m.group(4)),
            "s0_total_us": int(m.group(5)),
            "crypto_us": int(m.group(6)),
            "s1_total_us": int(m.group(7)),
            "full_s2_us": int(m.group(8)),
            "degraded": int(m.group(9)),
            "result": m.group(10),
            "epoch": int(m.group(11)) if m.group(11) else 0,
        })
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--disk", default="target/qemu/exofs-root.img")
    ap.add_argument("--iso", default="exo-os.iso")
    ap.add_argument("--release", action="store_true", help="Enforce strict KVM release absolute budgets")
    ap.add_argument("--smp", type=int, default=0, help="vCPU count (default: 2 with KVM, 1 with TCG)")
    args = ap.parse_args()

    os.chdir(REPO_ROOT)
    base_disk = Path(args.disk)
    iso_path = Path(args.iso)
    if not base_disk.exists() or not iso_path.exists():
        print(f"[ERROR] Missing disk ({base_disk}) or ISO ({iso_path})", file=sys.stderr)
        return 1

    has_kvm = os.path.exists("/dev/kvm") and os.access("/dev/kvm", os.R_OK | os.W_OK)
    smp = args.smp if args.smp > 0 else (2 if has_kvm else 1)
    pid = os.getpid()
    ephemeral_disk = Path(f"/tmp/exoos-phx-disk-{pid}.img")
    qmp_path = Path(f"/tmp/exoos-phx-qmp-{pid}.sock")
    serial_log = Path(f"/tmp/exoos-phx-serial-{pid}.log")
    e9_log = Path(f"/tmp/exoos-phx-e9-{pid}.log")
    qemu_out = Path(f"/tmp/exoos-phx-qemu-{pid}.log")

    shutil.copyfile(base_disk, ephemeral_disk)
    proc: subprocess.Popen[bytes] | None = None

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
        if has_kvm:
            qemu_cmd.extend(["-enable-kvm", "-cpu", "host"])
        print(f"[PHOENIX-BENCH] Launching QEMU (accel={'KVM' if has_kvm else 'TCG'})...", flush=True)

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
            raise TimeoutError(f"Timed out waiting for {needle!r}")

        def wait_for_prompt_count(target: int, timeout_s: float) -> bytes:
            deadline = time.time() + timeout_s
            while time.time() < deadline:
                data = e9_bytes()
                if prompt_count(data) >= target and input_prompt_ready(data):
                    return data
                time.sleep(0.25)
            tail = e9_bytes()[-2048:].decode("utf-8", "replace")
            print(f"[PHOENIX-BENCH TIMEOUT TAIL]\n{tail}", file=sys.stderr, flush=True)
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

        wait_for_e9(b"Exo-OS userspace console ready", 600)
        data = wait_for_e9(b"exosh:/", 120)
        seen_prompts = prompt_count(data)
        wait_for_prompt_count(seen_prompts, 120)

        # Warmup pass: prime QEMU TCG JIT + Golden RAM Mirror + host CPU governor before measuring
        print("[PHOENIX-BENCH] Running warmup pass (priming TCG JIT & Golden RAM Mirror)...", flush=True)
        send_text("/bin/exo-compliance-runner --phoenix-bench-a\n")
        seen_prompts += 1
        wait_for_prompt_count(seen_prompts, 180)

        # Interleaved A1 / B1 / A2 / B2 measurement eliminates host CPU/TCG thermal drift
        recs_a: list[dict] = []
        recs_b: list[dict] = []
        for idx in (1, 2):
            print(f"[PHOENIX-BENCH] Running Cas A round {idx} (Baseline Strate 1)...", flush=True)
            a_start = len(e9_bytes())
            send_text("/bin/exo-compliance-runner --phoenix-bench-a\n")
            seen_prompts += 1
            data = wait_for_prompt_count(seen_prompts, 180)
            recs_a.extend(parse_sla_records(data[a_start:].decode("utf-8", "replace")))

            print(f"[PHOENIX-BENCH] Running Cas B round {idx} (Full Strate 2 Load)...", flush=True)
            b_start = len(e9_bytes())
            send_text("/bin/exo-compliance-runner --phoenix-bench-b\n")
            seen_prompts += 1
            data = wait_for_prompt_count(seen_prompts, 180)
            recs_b.extend(parse_sla_records(data[b_start:].decode("utf-8", "replace")))

        # Verify ExoLedger chain post-switch via `exo audit --verify-chain`
        print("[PHOENIX-BENCH] Verifying ExoLedger chain (`exo audit --verify-chain`)...", flush=True)
        aud_start = len(e9_bytes())
        send_text("exo audit --verify-chain\n")
        seen_prompts += 1
        data = wait_for_prompt_count(seen_prompts, 120)
        aud_text = data[aud_start:].decode("utf-8", "replace")
        ledger_ok = "PASS" in aud_text or "INTACT" in aud_text

        sock.close()

        for r in recs_a:
            print(f"  [CAS A] {r}", flush=True)
        for r in recs_b:
            print(f"  [CAS B] {r}", flush=True)

        if not recs_a or not recs_b:
            print("[FAIL] Missing [EXO_PHOENIX_SLA] records!", file=sys.stderr)
            return 1

        med_a_s0 = statistics.median(r["s0_total_us"] for r in recs_a)
        med_a_s1 = statistics.median(r["s1_total_us"] for r in recs_a)
        med_b_s0 = statistics.median(r["s0_total_us"] for r in recs_b)
        med_b_s1 = statistics.median(r["s1_total_us"] for r in recs_b)
        drift_pct = (abs(med_b_s1 - med_a_s1) / max(med_a_s1, 1)) * 100.0

        all_clean = all(r["degraded"] == 0 and r["result"] == "PASS" for r in recs_a + recs_b)
        epochs_monotonic = recs_b[-1]["epoch"] > recs_a[0]["epoch"]
        invariance_ok = drift_pct <= 15.0
        abs_ok = True
        if has_kvm and args.release:
            abs_ok = (
                med_a_s0 <= 100_000
                and med_b_s0 <= 100_000
                and med_a_s1 <= 1_000_000
                and med_b_s1 <= 1_000_000
            )

        print(
            f"\n[PHOENIX-BENCH SUMMARY]\n"
            f"  Accel           : {'KVM' if has_kvm else 'TCG'}\n"
            f"  Cas A median    : S0={med_a_s0:.0f} us, S1_SLA={med_a_s1:.0f} us\n"
            f"  Cas B median    : S0={med_b_s0:.0f} us, S1_SLA={med_b_s1:.0f} us\n"
            f"  O(1) Drift      : {drift_pct:.2f}% (limit <= 15.00%) -> {'PASS' if invariance_ok else 'FAIL'}\n"
            f"  DEGRADED=0      : {'PASS' if all_clean else 'FAIL'}\n"
            f"  Epoch Monotonic : {'PASS' if epochs_monotonic else 'FAIL'} ({recs_a[0]['epoch']} -> {recs_b[-1]['epoch']})\n"
            f"  ExoLedger Chain : {'PASS' if ledger_ok else 'FAIL'}\n"
            f"  Absolute SLA    : {'PASS' if abs_ok else 'FAIL'} ({'enforced' if (has_kvm and args.release) else 'TCG/dev informational'})",
            flush=True,
        )

        ok = all_clean and epochs_monotonic and ledger_ok and invariance_ok and abs_ok
        return 0 if ok else 1
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


if __name__ == "__main__":
    sys.exit(main())
