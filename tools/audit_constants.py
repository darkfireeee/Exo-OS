#!/usr/bin/env python3
"""Audit critical Exo-OS constants for duplicated or forbidden values."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from dataclasses import asdict, dataclass
from pathlib import Path


CRITICAL_PATTERNS: list[tuple[str, str]] = [
    (r"MAX_CORES?(?:_LAYOUT|_RUNTIME)?", "kernel/src/arch/constants.rs"),
    (r"SSR_MAX_CORES(?:_LAYOUT|_RUNTIME)?", "kernel/src/arch/constants.rs"),
    (r"CORE_MASK_WORDS", "kernel/src/arch/constants.rs"),
    (r"MAX_MSG_SIZE", "kernel/src/arch/constants.rs"),
    (r"IPC_INLINE_MAX", "kernel/src/arch/constants.rs"),
    (r"MAX_PROCESSES?", "kernel/src/arch/constants.rs"),
    (r"MAX_ENDPOINTS", "kernel/src/arch/constants.rs"),
    (r"USER_ELF_BASE_MIN", "kernel/src/arch/constants.rs"),
    (r"KERNEL_BASE", "kernel/src/arch/constants.rs"),
    (r"KAIROS_WINDOW_NS", "kernel/src/arch/constants.rs"),
    (r"DECEPTION_THRESHOLD", "kernel/src/security/exoargos.rs"),
    (r"SSR_MAX_PROCESSES", "kernel/src/exophoenix/ssr.rs"),
    (r"SSR_MAX_ENDPOINTS", "kernel/src/exophoenix/ssr.rs"),
]

FORBIDDEN_VALUE_RULES: list[tuple[str, str, str]] = [
    (
        r".*(VIRTIO|MMIO|BAR).*",
        r"0x1000_0000|0x10000000",
        "hardcoded VirtIO/MMIO BAR address; use PCI config BAR discovery",
    ),
]

IPC_README_EXPECTED: dict[str, list[str]] = {
    "IPC_MAX_CHANNELS": ["65 536", "65536"],
    "IPC_MAX_ENDPOINTS": ["8 192", "8192"],
    "IPC_MAX_PROCESSES": ["65 536", "65536"],
    "MSG_HEADER_MAGIC": ["0x4D53_4748", "0x4D534748"],
    "IPC_FUTEX_MAGIC": ["0x1FCF_07E0", "0x1FCF07E0"],
    "SYNC_CHANNEL_TIMEOUT_NS": ["5 ms", "5 000 000 ns", "5000000"],
}


@dataclass(frozen=True)
class Finding:
    name: str
    file: str
    line: int
    value: str
    canonical_file: str


def compile_constant_pattern() -> re.Pattern[str]:
    names = "|".join(pattern for pattern, _ in CRITICAL_PATTERNS)
    return re.compile(
        rf"(?:pub\s+)?(?:const|static)\s+({names})"
        r"\s*:\s*[\w:<>,\[\];& ]+\s*=\s*([^;]+);",
        re.IGNORECASE | re.MULTILINE,
    )


def canonical_for(name: str) -> str:
    for pattern, canonical in CRITICAL_PATTERNS:
        if re.fullmatch(pattern, name, re.IGNORECASE):
            return canonical
    return "unknown"


def normalize_doc_value(value: str) -> str:
    return re.sub(r"[\s_`]", "", value).upper()


def audit_ipc_readme(repo_root: Path) -> list[dict]:
    errors: list[dict] = []
    readme = repo_root / "docs" / "kernel" / "ipc" / "README.md"
    if not readme.exists():
        errors.append(
            {
                "type": "MISSING_IPC_DOC",
                "file": readme.relative_to(repo_root).as_posix(),
                "reason": "docs/kernel/ipc/README.md not found",
            }
        )
        return errors

    rows: dict[str, str] = {}
    text = readme.read_text(encoding="utf-8", errors="replace")
    for line in text.splitlines():
        match = re.match(r"\|\s*`([^`]+)`\s*\|\s*([^|]+)\|", line)
        if match:
            rows[match.group(1)] = match.group(2).strip()

    for name, expected_values in IPC_README_EXPECTED.items():
        value = rows.get(name)
        if value is None:
            errors.append(
                {
                    "type": "MISSING_IPC_DOC_CONSTANT",
                    "constant": name,
                    "file": readme.relative_to(repo_root).as_posix(),
                    "reason": "constant absent from README key constants table",
                }
            )
            continue

        normalized = normalize_doc_value(value)
        if not any(normalize_doc_value(expected) in normalized for expected in expected_values):
            errors.append(
                {
                    "type": "STALE_IPC_DOC_CONSTANT",
                    "constant": name,
                    "file": readme.relative_to(repo_root).as_posix(),
                    "value": value,
                    "expected_any": expected_values,
                }
            )

    return errors


def audit(repo_root: Path) -> tuple[list[dict], list[dict], dict[str, list[Finding]]]:
    errors: list[dict] = []
    warnings: list[dict] = []
    findings_by_name: dict[str, list[Finding]] = defaultdict(list)
    pattern = compile_constant_pattern()

    kernel_root = repo_root / "kernel" / "src"
    if not kernel_root.exists():
        raise FileNotFoundError("kernel/src not found; run from the Exo-OS repository root")

    for rust_file in kernel_root.rglob("*.rs"):
        text = rust_file.read_text(encoding="utf-8", errors="replace")
        rel = rust_file.relative_to(repo_root).as_posix()
        for match in pattern.finditer(text):
            name = match.group(1)
            value = " ".join(match.group(2).strip().split())
            line = text[: match.start()].count("\n") + 1
            findings_by_name[name.upper()].append(
                Finding(name.upper(), rel, line, value, canonical_for(name))
            )

        for const_match in re.finditer(
            r"(?:pub\s+)?(?:const|static)\s+(\w+)\s*:[^=]+=\s*([^;]+);",
            text,
            re.MULTILINE,
        ):
            name = const_match.group(1)
            value = " ".join(const_match.group(2).strip().split())
            for name_rule, value_rule, reason in FORBIDDEN_VALUE_RULES:
                if re.fullmatch(name_rule, name, re.IGNORECASE) and re.search(
                    value_rule, value, re.IGNORECASE
                ):
                    errors.append(
                        {
                            "type": "FORBIDDEN_VALUE",
                            "constant": name,
                            "file": rel,
                            "line": text[: const_match.start()].count("\n") + 1,
                            "value": value,
                            "reason": reason,
                        }
                    )

    errors.extend(audit_ipc_readme(repo_root))

    for name, findings in sorted(findings_by_name.items()):
        values = {finding.value for finding in findings}
        if len(values) > 1:
            errors.append(
                {
                    "type": "INCOHERENT_CONSTANT",
                    "constant": name,
                    "values": sorted(values),
                    "locations": [asdict(finding) for finding in findings],
                }
            )
            continue

        if len(findings) > 1:
            canonical = findings[0].canonical_file
            duplicates = [
                finding
                for finding in findings
                if canonical != "unknown" and canonical not in finding.file
            ]
            if duplicates:
                warnings.append(
                    {
                        "type": "DUPLICATED_CONSTANT",
                        "constant": name,
                        "canonical_file": canonical,
                        "duplicates": [asdict(finding) for finding in duplicates],
                    }
                )

    return errors, warnings, findings_by_name


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fail-on-warn", action="store_true")
    parser.add_argument("--json", action="store_true", help="emit machine-readable output")
    parser.add_argument("--repo-root", default=".", type=Path)
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    try:
        errors, warnings, _ = audit(repo_root)
    except FileNotFoundError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps({"errors": errors, "warnings": warnings}, indent=2))
    else:
        print("Exo-OS constant audit")
        print(f"errors: {len(errors)}")
        for error in errors:
            print(f"- {error['type']}: {error.get('constant')} {error.get('file', '')}")
        print(f"warnings: {len(warnings)}")
        for warning in warnings:
            print(f"- {warning['type']}: {warning.get('constant')}")

    if errors or (args.fail_on_warn and warnings):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
