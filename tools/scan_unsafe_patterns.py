#!/usr/bin/env python3
"""
scan_unsafe_patterns.py — Scan unwrap/panic/debug_assert en production
ExoOS v0.2.0 — Strata

Détecte dans le code Rust (hors tests) :
  - unwrap()  en dehors de blocs #[cfg(test)] / mod tests
  - expect()  idem
  - panic!()  idem (sauf les boot-panics documentés)
  - debug_assert! dans des chemins critiques connus

Usage :
    python3 tools/scan_unsafe_patterns.py [--repo ROOT] [--dir kernel|servers|all]
                                          [--severity P0|P1|P2|all]

Retourne 0 si aucun P0/P1 trouvé, 1 sinon.
"""

import sys
import re
import pathlib
import argparse
from dataclasses import dataclass, field
from typing import List, Optional


# ──────────────────────────────────────────────────────────────────────────────
# Modèle
# ──────────────────────────────────────────────────────────────────────────────

@dataclass
class Finding:
    file: str
    line: int
    pattern: str
    context: str
    severity: str      # P0 / P1 / P2
    known_ok: bool = False
    note: str = ""


# ──────────────────────────────────────────────────────────────────────────────
# Exceptions connues (patterns dont on accepte la présence)
# ──────────────────────────────────────────────────────────────────────────────

KNOWN_OK_PATTERNS = [
    # Boot panics documentés — légitimes (OOM = arrêt irrécupérable)
    re.compile(r'panic!\("ipc shm pool allocation failed'),
    re.compile(r'panic!\("Emergency pool'),
    re.compile(r'panic!\("Cannot boot'),
    # unwrap() dans les macros de génération de table (statique, pas runtime)
    re.compile(r'const.*=.*unwrap\(\)'),
    # expect dans les tests d'intégration nommés
    re.compile(r'#\[test\]'),
]

# Répertoires et fichiers à exclure entièrement
EXCLUDE_DIRS = {
    "tests", "test", "benches", "bench", "examples"
}
EXCLUDE_FILE_SUFFIXES = [
    "_test.rs", "_tests.rs", "test_utils.rs"
]


# ──────────────────────────────────────────────────────────────────────────────
# Détection de blocs test
# ──────────────────────────────────────────────────────────────────────────────

def is_in_test_block(lines: List[str], line_idx: int) -> bool:
    """Heuristique : cherche #[test] ou mod tests dans les 50 lignes précédentes."""
    start = max(0, line_idx - 50)
    context_lines = lines[start:line_idx]
    depth = 0
    for l in reversed(context_lines):
        stripped = l.strip()
        depth   += stripped.count('}') - stripped.count('{')
        if "#[test]" in stripped:
            return True
        if re.search(r'\bmod\s+tests?\b', stripped):
            return True
        if "#[cfg(test)]" in stripped:
            return True
    return False


# ──────────────────────────────────────────────────────────────────────────────
# Scan d'un fichier
# ──────────────────────────────────────────────────────────────────────────────

PATTERNS = [
    # (regex, severity, label)
    (re.compile(r'\.unwrap\(\)'),         "P1", "unwrap()"),
    (re.compile(r'\.expect\('),           "P1", "expect()"),
    (re.compile(r'\bpanic!\s*\('),        "P1", "panic!"),
    (re.compile(r'\bdebug_assert!\s*\('), "P2", "debug_assert!"),
]

# Patterns P0 : unwrap dans du code particulièrement critique
P0_PATHS = [
    "kernel/src/memory",
    "kernel/src/security",
    "kernel/src/ipc",
    "kernel/src/syscall",
    "kernel/src/exophoenix",
    "kernel/src/scheduler",
]

def scan_file(path: pathlib.Path, repo_root: pathlib.Path) -> List[Finding]:
    rel = str(path.relative_to(repo_root))

    # Exclure par suffixe
    if any(path.name.endswith(s) for s in EXCLUDE_FILE_SUFFIXES):
        return []
    # Exclure par répertoire
    parts = set(path.parts)
    if parts & EXCLUDE_DIRS:
        return []

    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return []

    lines  = text.splitlines()
    findings: List[Finding] = []

    for line_idx, line in enumerate(lines):
        stripped = line.strip()

        # Ignorer les lignes de commentaire pures
        if stripped.startswith("//") or stripped.startswith("/*") or stripped.startswith("*"):
            continue

        for (pattern, base_severity, label) in PATTERNS:
            if not pattern.search(line):
                continue

            # Skip si dans un bloc test
            if is_in_test_block(lines, line_idx):
                continue

            # Vérifier si c'est un known_ok
            known = any(ok.search(line) for ok in KNOWN_OK_PATTERNS)

            # Escalader à P0 pour les modules très critiques
            severity = base_severity
            if not known and label in ("unwrap()", "panic!"):
                if any(rel.startswith(p) for p in P0_PATHS):
                    severity = "P0"

            context_start = max(0, line_idx - 1)
            context_end   = min(len(lines), line_idx + 2)
            ctx = "\n".join(
                f"  {context_start+i+1:4d}| {l}"
                for i, l in enumerate(lines[context_start:context_end])
            )

            findings.append(Finding(
                file     = rel,
                line     = line_idx + 1,
                pattern  = label,
                context  = ctx,
                severity = severity,
                known_ok = known,
            ))

    return findings


# ──────────────────────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────────────────────

SEV_COLOR = {"P0": "\033[31m", "P1": "\033[33m", "P2": "\033[36m"}
RESET     = "\033[0m"
BOLD      = "\033[1m"


def main():
    parser = argparse.ArgumentParser(
        description="Scan unwrap/panic/debug_assert hors tests"
    )
    parser.add_argument("--repo",     default=".",
                        help="Racine du dépôt")
    parser.add_argument("--dir",      default="kernel",
                        choices=["kernel", "servers", "all"],
                        help="Sous-répertoire à scanner (défaut: kernel)")
    parser.add_argument("--severity", default="P1",
                        choices=["P0", "P1", "P2", "all"],
                        help="Afficher seulement <= cette sévérité (défaut: P1)")
    parser.add_argument("--show-known-ok", action="store_true",
                        help="Afficher aussi les occurrences connues-OK")
    args = parser.parse_args()

    repo   = pathlib.Path(args.repo).resolve()
    dirs   = (["kernel", "servers"] if args.dir == "all"
              else [args.dir])

    sev_threshold = {"P0": 0, "P1": 1, "P2": 2}
    show_up_to    = sev_threshold.get(args.severity, 2)

    all_findings: List[Finding] = []
    files_scanned = 0

    for d in dirs:
        base = repo / d
        if not base.is_dir():
            continue
        for rs_file in base.rglob("*.rs"):
            findings = scan_file(rs_file, repo)
            all_findings.extend(findings)
            files_scanned += 1

    # Filtrer
    visible = [
        f for f in all_findings
        if sev_threshold[f.severity] <= show_up_to
        and (args.show_known_ok or not f.known_ok)
    ]

    # Trier : P0 d'abord, puis P1, P2 ; puis par fichier
    visible.sort(key=lambda f: (sev_threshold[f.severity], f.file, f.line))

    print(f"\n{BOLD}ExoOS — scan_unsafe_patterns{RESET}")
    print(f"Repo    : {repo}")
    print(f"Dirs    : {dirs}")
    print(f"Fichiers: {files_scanned}")
    print(f"{'─'*70}\n")

    counts = {"P0": 0, "P1": 0, "P2": 0}
    for f in visible:
        counts[f.severity] += 1
        color = SEV_COLOR.get(f.severity, "")
        ok_tag = " [known-ok]" if f.known_ok else ""
        print(f"{color}[{f.severity}]{RESET} {f.file}:{f.line}  {BOLD}{f.pattern}{RESET}{ok_tag}")
        print(f"{f.context}\n")

    print(f"{'─'*70}")
    print(f"Total visible : {len(visible)}  "
          f"({SEV_COLOR['P0']}P0={counts['P0']}{RESET}  "
          f"{SEV_COLOR['P1']}P1={counts['P1']}{RESET}  "
          f"{SEV_COLOR['P2']}P2={counts['P2']}{RESET})")
    print(f"Total all_findings : {len(all_findings)} "
          f"(dont {sum(1 for f in all_findings if f.known_ok)} known-ok)\n")

    critical = counts["P0"] + counts["P1"]
    if critical > 0:
        print(f"\033[31m{critical} occurrences P0/P1 à corriger avant release.\033[0m")
        return 1
    print(f"\033[32mAucun P0/P1 détecté dans la sélection.\033[0m")
    return 0


if __name__ == "__main__":
    sys.exit(main())
