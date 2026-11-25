#!/usr/bin/env python3
"""
check_arch_asm_border.py — Enforces strict ISA isolation inside the Ring 0 kernel:
ZERO `asm!`, `global_asm!`, or `naked_asm!` invocations outside `kernel/src/arch/`.
"""

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
ASM_RE = re.compile(r"\b(?:core::arch::|std::arch::)?(?:global_asm|naked_asm|asm)!\s*\(")


def strip_comments_and_strings(src: str) -> str:
    # Remove line comments and block comments conservatively while preserving newlines
    out = []
    i = 0
    n = len(src)
    block_depth = 0
    in_str = False
    while i < n:
        ch = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if block_depth > 0:
            if ch == "/" and nxt == "*":
                block_depth += 1
                i += 2
            elif ch == "*" and nxt == "/":
                block_depth -= 1
                i += 2
            else:
                if ch == "\n":
                    out.append("\n")
                i += 1
            continue
        if in_str:
            if ch == "\\" and i + 1 < n:
                i += 2
            elif ch == '"':
                in_str = False
                i += 1
            else:
                if ch == "\n":
                    out.append("\n")
                i += 1
            continue
        if ch == "/" and nxt == "/":
            while i < n and src[i] != "\n":
                i += 1
            continue
        if ch == "/" and nxt == "*":
            block_depth = 1
            i += 2
            continue
        if ch == '"':
            in_str = True
            i += 1
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def main() -> int:
    kernel_src = REPO / "kernel/src"
    violations: list[str] = []
    scanned = 0

    for rs in sorted(kernel_src.rglob("*.rs")):
        rel = rs.relative_to(REPO).as_posix()
        if rel.startswith("kernel/src/arch/"):
            continue
        scanned += 1
        cleaned = strip_comments_and_strings(rs.read_text(encoding="utf-8", errors="replace"))
        for lineno, line in enumerate(cleaned.splitlines(), start=1):
            if ASM_RE.search(line):
                violations.append(f"{rel}:{lineno}: {line.strip()}")

    if violations:
        print(f"FAIL check_arch_asm_border: {len(violations)} inline asm site(s) outside kernel/src/arch/:")
        for v in violations:
            print(f"  - {v}")
        return 1

    print(f"OK check_arch_asm_border: {scanned} non-arch kernel files scanned, 0 inline asm violations.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
