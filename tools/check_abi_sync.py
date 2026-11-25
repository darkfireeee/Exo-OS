#!/usr/bin/env python3
"""
check_abi_sync.py — Verifies that `libs/exo-abi` is the single canonical definition
of Exo-OS syscall numbers, errno constants, and `CapObjectType` discriminants.

Checks:
  1. `SYS_*` constants in `library/std/src/sys/pal/exo_os/syscall.rs` and
     `libs/wayland-rs/exo-rustix/src/lib.rs` match `libs/exo-abi/src/numbers.rs`.
  2. Core capability discriminants in `library/std/src/os/exo/mod.rs` (`CapabilityType`)
     match `libs/exo-abi/src/cap.rs` (`CapObjectType`).
  3. Zero duplicate `const SYS_* = <literal>;` definitions outside `libs/exo-abi`
     and the audited sysroot/vendor mirrors.
"""

import os
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent

SYS_RE = re.compile(r"^\s*(?:pub\s+)?const\s+(SYS_[A-Z0-9_]+)\s*:\s*\w+\s*=\s*([0-9_]+)\s*;", re.M)
CAP_VARIANT_RE = re.compile(r"^\s*([A-Za-z0-9_]+)\s*=\s*([0-9_]+)\s*,", re.M)

ALLOWED_SYS_DEF_PREFIXES = (
    "libs/exo-abi/",
    "library/std/",
    "libs/vendors/",
    "libs/wayland-rs/",
    "libs/musl-exo/",
)

SKIP_DIRS = {"target", "vendors", "wayland-rs", "musl-exo", "library", ".git"}

# Canonical (exo-abi CapObjectType variant, std CapabilityType variant) pairs that MUST stay identical.
CORE_CAP_PAIRS = (
    ("Null", "Invalid"),
    ("IpcEndpoint", "IpcEndpoint"),
    ("MemoryRegion", "MemoryPage"),
    ("FileObject", "FileObject"),
    ("DeviceBar", "DeviceMmio"),
    ("IrqLine", "IrqLine"),
)


def parse_sys_consts(path: pathlib.Path) -> dict[str, int]:
    text = path.read_text(encoding="utf-8", errors="replace")
    return {name: int(val.replace("_", "")) for name, val in SYS_RE.findall(text)}


def parse_enum_variants(path: pathlib.Path, enum_name: str) -> dict[str, int]:
    text = path.read_text(encoding="utf-8", errors="replace")
    m = re.search(rf"pub\s+enum\s+{re.escape(enum_name)}\s*\{{(.*?)\}}", text, re.S)
    if not m:
        return {}
    return {name: int(val.replace("_", "")) for name, val in CAP_VARIANT_RE.findall(m.group(1))}


def check_mirror(name: str, path: pathlib.Path, canonical: dict[str, int], errors: list[str]) -> int:
    if not path.exists():
        errors.append(f"Missing ABI mirror file: {name}")
        return 0
    mirror = parse_sys_consts(path)
    for const_name, val in sorted(mirror.items()):
        if const_name in canonical and canonical[const_name] != val:
            errors.append(
                f"ABI drift: {const_name} is {val} in {name} vs {canonical[const_name]} in libs/exo-abi"
            )
    return len(mirror)


def main() -> int:
    errors: list[str] = []

    abi_numbers = REPO / "libs/exo-abi/src/numbers.rs"
    abi_cap = REPO / "libs/exo-abi/src/cap.rs"
    std_syscall = REPO / "library/std/src/sys/pal/exo_os/syscall.rs"
    std_exo_mod = REPO / "library/std/src/os/exo/mod.rs"
    rustix_lib = REPO / "libs/wayland-rs/exo-rustix/src/lib.rs"

    canonical_sys = parse_sys_consts(abi_numbers)
    if len(canonical_sys) < 100:
        errors.append(f"libs/exo-abi/src/numbers.rs has too few SYS_* definitions ({len(canonical_sys)})")

    std_checked = check_mirror("library/std/.../syscall.rs", std_syscall, canonical_sys, errors)
    rustix_checked = check_mirror("libs/wayland-rs/exo-rustix/src/lib.rs", rustix_lib, canonical_sys, errors)

    abi_caps = parse_enum_variants(abi_cap, "CapObjectType")
    std_caps = parse_enum_variants(std_exo_mod, "CapabilityType")
    if not abi_caps or not std_caps:
        errors.append("Missing CapObjectType in libs/exo-abi/src/cap.rs or CapabilityType in library/std/src/os/exo/mod.rs")
    else:
        for abi_v, std_v in CORE_CAP_PAIRS:
            if abi_v not in abi_caps:
                errors.append(f"CapObjectType::{abi_v} missing in libs/exo-abi/src/cap.rs")
            elif std_v not in std_caps:
                errors.append(f"CapabilityType::{std_v} missing in library/std/src/os/exo/mod.rs")
            elif abi_caps[abi_v] != std_caps[std_v]:
                errors.append(
                    f"Capability discriminant drift: CapObjectType::{abi_v}={abi_caps[abi_v]} "
                    f"vs std CapabilityType::{std_v}={std_caps[std_v]}"
                )

    # Reject duplicate literal `const SYS_* = <num>;` definitions outside allowlisted mirrors
    for root in ("kernel/src", "servers", "drivers", "loader/src", "userspace", "tests", "libs"):
        base = REPO / root
        if not base.exists():
            continue
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for fn in filenames:
                if not fn.endswith(".rs"):
                    continue
                rs = pathlib.Path(dirpath) / fn
                rel = rs.relative_to(REPO).as_posix()
                if rel.startswith(ALLOWED_SYS_DEF_PREFIXES):
                    continue
                found = SYS_RE.findall(rs.read_text(encoding="utf-8", errors="replace"))
                if found:
                    names = ", ".join(k for k, _ in found[:5])
                    errors.append(f"Duplicate SYS_* definition in {rel}: {names} ({len(found)} total)")

    if errors:
        print("FAIL check_abi_sync:")
        for e in errors:
            print(f"  - {e}")
        return 1

    print(
        f"OK check_abi_sync: {len(canonical_sys)} canonical SYS_*, "
        f"{std_checked} std + {rustix_checked} exo-rustix mirror checks, "
        f"{len(CORE_CAP_PAIRS)} core capability discriminants aligned, 0 rogue SYS_* defs."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
