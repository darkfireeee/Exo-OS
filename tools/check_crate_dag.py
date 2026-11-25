#!/usr/bin/env python3
"""
check_crate_dag.py — Enforces the acyclic crate dependency DAG across all 6 Cargo workspaces.

Rules enforced:
  1. No `libs/*`, `kernel/`, `exo-boot/`, or `drivers/*` manifest may depend by path on `servers/*`.
  2. No `kernel/`, `exo-boot/`, or `tools/*` manifest may depend by path on `drivers/*`,
     except the single explicit boot-disk allowlist (`kernel/Cargo.toml` -> `drivers/storage/virtio_blk`).
  3. No `#[path = "..."]` attribute in Rust sources may cross a `Cargo.toml` crate boundary.
"""

import os
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent

PATH_DEP_RE = re.compile(r'path\s*=\s*"([^"]+)"')
ATTR_PATH_RE = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]')

ALLOWED_DRIVER_DEPS = {
    ("kernel/Cargo.toml", "drivers/storage/virtio_blk"),
}

SKIP_DIRS = {"target", "vendors", "wayland-rs", "musl-exo", "library", ".git"}


def find_crate_root(file_path: pathlib.Path) -> pathlib.Path | None:
    cur = file_path.parent
    while True:
        if (cur / "Cargo.toml").exists():
            return cur
        if cur == REPO or cur.parent == cur:
            return None
        cur = cur.parent


def main() -> int:
    errors: list[str] = []
    manifests_checked = 0

    for dirpath, dirnames, filenames in os.walk(REPO):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        if "Cargo.toml" not in filenames:
            continue
        toml = pathlib.Path(dirpath) / "Cargo.toml"
        rel = toml.relative_to(REPO).as_posix()
        manifests_checked += 1
        text = toml.read_text(encoding="utf-8", errors="replace")
        for line in text.splitlines():
            stripped = line.strip()
            if (
                stripped.startswith("#")
                or stripped.startswith('path = "src/')
                or stripped.startswith('path = "tests/')
            ):
                continue
            for m in PATH_DEP_RE.finditer(line):
                raw_target = m.group(1)
                if raw_target.endswith(".rs"):
                    continue
                resolved = (toml.parent / raw_target).resolve()
                try:
                    target_rel = resolved.relative_to(REPO).as_posix()
                except ValueError:
                    continue

                # Rule 1: libs/*, kernel, exo-boot, drivers/* -> servers/* forbidden
                if (
                    rel.startswith(("libs/", "kernel/", "exo-boot/", "drivers/"))
                    and target_rel.startswith("servers/")
                ):
                    errors.append(f"Inverted dependency: {rel} -> {target_rel}")

                # Rule 2: kernel, exo-boot, tools/* -> drivers/* forbidden (except allowlist)
                if (
                    rel.startswith(("kernel/", "exo-boot/", "tools/"))
                    and target_rel.startswith("drivers/")
                    and (rel, target_rel) not in ALLOWED_DRIVER_DEPS
                ):
                    errors.append(f"Disallowed driver dependency: {rel} -> {target_rel}")

    # Rule 3: No cross-crate #[path = "..."]
    for root in ("kernel/src", "exo-boot/src", "loader/src", "servers", "drivers", "libs", "userspace", "tests"):
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
                crate_root = find_crate_root(rs)
                if crate_root is None:
                    continue
                txt = rs.read_text(encoding="utf-8", errors="replace")
                for m in ATTR_PATH_RE.finditer(txt):
                    target = (rs.parent / m.group(1)).resolve()
                    try:
                        target.relative_to(crate_root)
                    except ValueError:
                        try:
                            t_rel = target.relative_to(REPO).as_posix()
                        except ValueError:
                            t_rel = str(target)
                        errors.append(f"Cross-crate #[path] in {rel} -> {t_rel}")

    if errors:
        print("FAIL check_crate_dag:")
        for e in errors:
            print(f"  - {e}")
        return 1

    print(f"OK check_crate_dag: {manifests_checked} Cargo manifests verified, 0 inversions, 0 cross-crate #[path].")
    return 0


if __name__ == "__main__":
    sys.exit(main())
