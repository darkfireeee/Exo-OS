#!/usr/bin/env python3
"""
scan_unsafe_contracts.py — Détecte les blocs `unsafe {` sans contrat `// SAFETY:`
ExoOS — RÈGLE CONTRAT UNSAFE (regle_bonus.md)

Pour chaque occurrence de `unsafe` ouvrant un bloc (hors `unsafe fn`/`unsafe trait`/
`unsafe impl`), on exige un commentaire `// SAFETY:` dans les `--window` lignes
précédentes (défaut 4). Les blocs en #[cfg(test)] sont ignorés.

Usage :
    python3 tools/scan_unsafe_contracts.py [--repo ROOT] [--dir kernel|servers|all]
                                           [--window N] [--list] [--by-file]
Retourne le nombre de blocs non documentés (0 = tout est documenté).
"""

import sys, re, pathlib, argparse
from collections import Counter

# `unsafe {` ou `unsafe {` en fin de ligne, mais PAS `unsafe fn/trait/impl`
UNSAFE_BLOCK = re.compile(r'(^|[^\w])unsafe\s*\{')
UNSAFE_DECL  = re.compile(r'\bunsafe\s+(fn|trait|impl)\b')
SAFETY_RE    = re.compile(r'(//|/\*|\*)\s*SAFETY', re.IGNORECASE)

EXCLUDE_FILE_SUFFIXES = ["_test.rs", "_tests.rs", "test_utils.rs"]
EXCLUDE_DIRS = {"tests", "test", "benches", "bench", "examples", "vendors",
                "hickory-dns-upstream", "smoltcp-upstream"}


def is_in_test_block(lines, idx):
    start = max(0, idx - 60)
    for l in reversed(lines[start:idx]):
        s = l.strip()
        if "#[test]" in s or "#[cfg(test)]" in s or re.search(r'\bmod\s+tests?\b', s):
            return True
    return False


def scan_file(path, repo, window):
    rel = str(path.relative_to(repo))
    if any(path.name.endswith(s) for s in EXCLUDE_FILE_SUFFIXES):
        return []
    if set(path.parts) & EXCLUDE_DIRS:
        return []
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return []
    lines = text.splitlines()
    findings = []
    for i, line in enumerate(lines):
        if not UNSAFE_BLOCK.search(line):
            continue
        if UNSAFE_DECL.search(line):
            continue
        if is_in_test_block(lines, i):
            continue
        # Chercher un // SAFETY: dans la fenêtre précédente (ou sur la ligne)
        ctx = lines[max(0, i - window):i + 1]
        if any(SAFETY_RE.search(c) for c in ctx):
            continue
        findings.append((rel, i + 1, line.strip()))
    return findings


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--repo", default=".")
    p.add_argument("--dir", default="kernel", choices=["kernel", "servers", "all"])
    p.add_argument("--window", type=int, default=4)
    p.add_argument("--list", action="store_true", help="lister chaque occurrence")
    p.add_argument("--by-file", action="store_true", help="compter par fichier")
    args = p.parse_args()

    repo = pathlib.Path(args.repo).resolve()
    dirs = (["kernel", "servers"] if args.dir == "all" else [args.dir])

    all_f = []
    for d in dirs:
        base = repo / d
        if not base.is_dir():
            continue
        for rs in base.rglob("*.rs"):
            all_f.extend(scan_file(rs, repo, args.window))

    if args.by_file:
        c = Counter(f[0] for f in all_f)
        for name, n in c.most_common(40):
            print(f"{n:4d}  {name}")
        print(f"{'-'*50}\nTotal fichiers: {len(c)}  Total blocs: {len(all_f)}")
    elif args.list:
        for rel, ln, src in all_f:
            print(f"{rel}:{ln}: {src}")
        print(f"{'-'*50}\nTotal: {len(all_f)}")
    else:
        print(f"unsafe sans // SAFETY: {len(all_f)} (dirs={dirs}, window={args.window})")

    return 0 if not all_f else len(all_f)


if __name__ == "__main__":
    sys.exit(main())
