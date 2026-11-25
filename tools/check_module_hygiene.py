#!/usr/bin/env python3
"""
check_module_hygiene.py — Permanent structural hygiene guard:
  1. Zero raw binary assets (`.bin`, `.img`, `.weights`) under any `**/src/**` tree.
  2. Zero non-test `use super::*;` glob imports inside `kernel/src/syscall/` or `kernel/src/fs/`
     (with a closed non-growing ratchet for legacy `fs_bridge` / `vfs_compat` submodules).
  3. Every non-vendored `mod.rs` <= 150 lines (with a closed non-growing ceiling ratchet
     for pre-existing legacy facades).
"""

import os
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent

MAX_MOD_LINES = 150
BINARY_EXTS = {".bin", ".img", ".weights"}
SUPER_GLOB_RE = re.compile(r"^\s*use\s+super::\*;", re.M)

# Closed non-growing ceiling ratchet for pre-existing legacy mod.rs facades.
# No new file may ever be added here; counts may only decrease toward <= 150.
LEGACY_MOD_CEILING = {
    "exo-boot/src/bios/mod.rs": 203,
    "exo-boot/src/kernel_loader/mod.rs": 159,
    "kernel/src/arch/aarch64/mod.rs": 156,
    "kernel/src/arch/x86_64/mod.rs": 434,
    "kernel/src/arch/x86_64/time/mod.rs": 269,
    "kernel/src/arch/x86_64/time/sources/mod.rs": 396,
    "kernel/src/drivers/mod.rs": 357,
    "kernel/src/exophoenix/stage0/mod.rs": 222,
    "kernel/src/ipc/mod.rs": 252,
    "kernel/src/memory/mod.rs": 183,
    "kernel/src/memory/physical/allocator/buddy/mod.rs": 304,
    "kernel/src/memory/physical/zone/mod.rs": 209,
    "kernel/src/process/mod.rs": 178,
    "kernel/src/syscall/dispatch/mod.rs": 357,
    "kernel/src/syscall/mod.rs": 298,
    "kernel/src/syscall/table/mod.rs": 295,
    "libs/exo-libc/src/platform/exo_os/mod.rs": 319,
    "loader/src/dynamic_linker/mod.rs": 732,
}

# Closed non-growing ratchet for legacy non-test `use super::*;` in internal split modules.
LEGACY_SUPER_GLOB_ALLOWLIST = {
    "kernel/src/syscall/fs_bridge/cwd.rs",
    "kernel/src/syscall/fs_bridge/fd.rs",
    "kernel/src/syscall/fs_bridge/file_ops.rs",
    "kernel/src/syscall/fs_bridge/poll.rs",
    "kernel/src/syscall/fs_bridge/types.rs",
    "kernel/src/fs/exofs/posix_bridge/vfs_compat/directory.rs",
    "kernel/src/fs/exofs/posix_bridge/vfs_compat/fd_table.rs",
    "kernel/src/fs/exofs/posix_bridge/vfs_compat/helpers.rs",
    "kernel/src/fs/exofs/posix_bridge/vfs_compat/ops.rs",
    "kernel/src/fs/exofs/posix_bridge/vfs_compat/persist.rs",
}

SKIP_DIRS = {"target", "vendors", "wayland-rs", "musl-exo", "library", ".git", "target-linux"}


def main() -> int:
    errors: list[str] = []
    mod_checked = 0

    for dirpath, dirnames, filenames in os.walk(REPO):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        cur = pathlib.Path(dirpath)
        for fn in sorted(filenames):
            path = cur / fn
            rel = path.relative_to(REPO)
            rel_posix = rel.as_posix()

            # 1. Binary assets under **/src/**
            if "src" in rel.parts and path.suffix.lower() in BINARY_EXTS:
                errors.append(f"Binary blob committed under src/: {rel_posix}")

            # 2. Non-test `use super::*;` forbidden in kernel/src/syscall/ and kernel/src/fs/
            if (
                path.suffix == ".rs"
                and fn != "tests.rs"
                and rel_posix not in LEGACY_SUPER_GLOB_ALLOWLIST
                and (
                    rel_posix.startswith("kernel/src/syscall/")
                    or rel_posix.startswith("kernel/src/fs/")
                )
            ):
                txt = path.read_text(encoding="utf-8", errors="replace")
                non_test_head = txt.split("#[cfg(test)]")[0]
                if SUPER_GLOB_RE.search(non_test_head):
                    errors.append(f"Disallowed non-test `use super::*;` glob import in {rel_posix}")

            # 3. `mod.rs` line budget
            if fn == "mod.rs":
                mod_checked += 1
                lines = len(path.read_text(encoding="utf-8", errors="replace").splitlines())
                limit = LEGACY_MOD_CEILING.get(rel_posix, MAX_MOD_LINES)
                if lines > limit:
                    errors.append(f"Oversized mod.rs: {rel_posix} has {lines} lines (limit {limit})")

    if errors:
        print("FAIL check_module_hygiene:")
        for e in errors:
            print(f"  - {e}")
        return 1

    print(
        f"OK check_module_hygiene: {mod_checked} mod.rs files within budget, "
        f"0 binary blobs in src/, 0 new non-test `use super::*;` in kernel syscall/fs."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
