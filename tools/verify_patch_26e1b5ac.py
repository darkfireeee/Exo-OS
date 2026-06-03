#!/usr/bin/env python3
"""
verify_patch_26e1b5ac.py — Vérification des patches du commit 26e1b5ac
ExoOS v0.2.0 — Strata

Usage :
    python3 tools/verify_patch_26e1b5ac.py [--repo ROOT] [--verbose]

    ROOT  : racine du dépôt ExoOS (défaut : répertoire courant)
    Retourne exit code 0 si tous les checks passent, 1 sinon.
"""

import sys
import re
import pathlib
import argparse
from dataclasses import dataclass, field
from typing import Callable, List

# ──────────────────────────────────────────────────────────────────────────────
# Infrastructure de checks
# ──────────────────────────────────────────────────────────────────────────────

@dataclass
class CheckResult:
    name: str
    passed: bool
    message: str
    severity: str = "P1"   # P0 / P1 / P2


def check(name: str, severity: str = "P1"):
    """Décorateur enregistrant un check dans le registre global."""
    def decorator(fn: Callable):
        fn._check_name     = name
        fn._check_severity = severity
        return fn
    return decorator


# ──────────────────────────────────────────────────────────────────────────────
# Helpers lecture fichier
# ──────────────────────────────────────────────────────────────────────────────

def read(root: pathlib.Path, rel: str) -> str:
    p = root / rel
    if not p.exists():
        return ""
    return p.read_text(encoding="utf-8", errors="replace")


def file_exists(root: pathlib.Path, rel: str) -> bool:
    return (root / rel).exists()


# ──────────────────────────────────────────────────────────────────────────────
# CHECKS — Sécurité (P0 / P1)
# ──────────────────────────────────────────────────────────────────────────────

@check("SEC-01a : exo_shield dans DEPS_EXOSH", severity="P0")
def check_exosh_requires_shield(root: pathlib.Path) -> CheckResult:
    src = read(root, "servers/init_server/src/service_table.rs")
    if not src:
        return CheckResult("SEC-01a", False,
                           "service_table.rs introuvable", "P0")

    # DEPS_EXOSH doit contenir "exo_shield"
    deps_exosh_block = re.search(
        r'const DEPS_EXOSH\s*:[^=]+=\s*&\[([^\]]*)\]', src, re.DOTALL
    )
    if not deps_exosh_block:
        return CheckResult("SEC-01a", False,
                           "Constante DEPS_EXOSH non trouvee", "P0")

    content = deps_exosh_block.group(1)
    if '"exo_shield"' in content:
        return CheckResult("SEC-01a", True,
                           "DEPS_EXOSH contient 'exo_shield' ✓")
    return CheckResult("SEC-01a", False,
                       f"DEPS_EXOSH ne contient pas 'exo_shield'.\n"
                       f"  Contenu: {content.strip()!r}", "P0")


@check("SEC-01b : exosh absent de DEPS_EXO_SHIELD", severity="P0")
def check_shield_not_requires_exosh(root: pathlib.Path) -> CheckResult:
    src = read(root, "servers/init_server/src/service_table.rs")
    if not src:
        return CheckResult("SEC-01b", False,
                           "service_table.rs introuvable", "P0")

    deps_shield_block = re.search(
        r'const DEPS_EXO_SHIELD\s*:[^=]+=\s*&\[([^\]]*)\]', src, re.DOTALL
    )
    if not deps_shield_block:
        return CheckResult("SEC-01b", False,
                           "Constante DEPS_EXO_SHIELD non trouvee", "P0")

    content = deps_shield_block.group(1)
    # "exosh" ne doit PAS apparaître (hors commentaire)
    lines = [l for l in content.splitlines() if not l.strip().startswith("//")]
    active = "\n".join(lines)
    if '"exosh"' not in active:
        return CheckResult("SEC-01b", True,
                           "DEPS_EXO_SHIELD ne contient pas 'exosh' ✓")
    return CheckResult("SEC-01b", False,
                       f"DEPS_EXO_SHIELD contient encore 'exosh'.\n"
                       f"  Lignes actives: {active.strip()!r}", "P0")


@check("SEC-01c : exo_shield avant exosh dans CANONICAL_SERVICES", severity="P0")
def check_canonical_order(root: pathlib.Path) -> CheckResult:
    src = read(root, "servers/init_server/src/service_table.rs")
    if not src:
        return CheckResult("SEC-01c", False,
                           "service_table.rs introuvable", "P0")

    # Trouver les positions des deux noms dans CANONICAL_SERVICES
    canon_start = src.find("CANONICAL_SERVICES")
    if canon_start == -1:
        return CheckResult("SEC-01c", False, "CANONICAL_SERVICES non trouve", "P0")

    canon_block = src[canon_start:]
    pos_shield = canon_block.find('"exo_shield"')
    pos_exosh  = canon_block.find('"exosh"')

    if pos_shield == -1 or pos_exosh == -1:
        return CheckResult("SEC-01c", False,
                           f"Entrées manquantes: shield@{pos_shield} exosh@{pos_exosh}",
                           "P0")

    if pos_shield < pos_exosh:
        return CheckResult("SEC-01c", True,
                           "exo_shield (vague 5) < exosh (vague 6) ✓")
    return CheckResult("SEC-01c", False,
                       f"ORDRE INVERSE: exosh ({pos_exosh}) avant "
                       f"exo_shield ({pos_shield}) dans CANONICAL_SERVICES",
                       "P0")


@check("SEC-02 : SYS_FRAMEBUFFER_INFO — FB_INFO_AUTHORIZED_PID guard", severity="P1")
def check_fb_info_guard(root: pathlib.Path) -> CheckResult:
    src = read(root, "kernel/src/syscall/table.rs")
    if not src:
        return CheckResult("SEC-02", False,
                           "kernel/src/syscall/table.rs introuvable", "P1")

    checks = [
        ("FB_INFO_AUTHORIZED_PID",   "AtomicU32 de controle absent"),
        ("compare_exchange",          "compare_exchange absent (first-caller guard)"),
        ("EACCES",                    "retour EACCES absent"),
    ]
    for token, msg in checks:
        fn_block = _extract_fn_block(src, "sys_framebuffer_info")
        if fn_block and token in fn_block:
            continue
        # Chercher dans tout le fichier (la static peut être déclarée juste avant)
        # On cherche dans les 100 lignes précédant la fonction
        fn_pos = src.find("pub fn sys_framebuffer_info")
        context = src[max(0, fn_pos - 800):fn_pos + 1200] if fn_pos != -1 else ""
        if token not in context:
            return CheckResult("SEC-02", False, msg, "P1")

    return CheckResult("SEC-02", True,
                       "sys_framebuffer_info protege par FB_INFO_AUTHORIZED_PID ✓")


def _extract_fn_block(src: str, fn_name: str) -> str:
    """Extrait le corps d'une fonction Rust (heuristique brace counting).
    Gere les generiques: fn foo<T>(  et  fn foo(
    """
    # Recherche souple : fn <name> suivi de < ou ( (generiques possibles)
    m = re.search(r'\bfn \s*' + re.escape(fn_name) + r'[\s<(]', src)
    if not m:
        return ""
    start = m.start()
    depth = 0
    begun = False
    result = []
    for ch in src[start:]:
        result.append(ch)
        if ch == '{':
            depth += 1
            begun = True
        elif ch == '}':
            depth -= 1
            if begun and depth == 0:
                break
    return "".join(result)


# ──────────────────────────────────────────────────────────────────────────────
# CHECKS — Mémoire / Robustesse (P1)
# ──────────────────────────────────────────────────────────────────────────────

@check("MEM-01 : map_page_unflushed — assert! dur (pas debug_assert!)", severity="P1")
def check_map_page_assert(root: pathlib.Path) -> CheckResult:
    src = read(root, "kernel/src/memory/virtual/address_space/user.rs")
    if not src:
        return CheckResult("MEM-01", False, "user.rs introuvable", "P1")

    fn_block = _extract_fn_block(src, "map_page_unflushed")
    if not fn_block:
        return CheckResult("MEM-01", False,
                           "Fonction map_page_unflushed non trouvee", "P1")

    if "debug_assert!" in fn_block and "USER_END" in fn_block:
        return CheckResult("MEM-01", False,
                           "debug_assert! encore present dans map_page_unflushed "
                           "(sera supprime en release build)", "P1")

    if "assert!" in fn_block and "USER_END" in fn_block:
        return CheckResult("MEM-01", True,
                           "assert! dur present dans map_page_unflushed ✓")

    return CheckResult("MEM-01", False,
                       "Aucune assertion USER_END dans map_page_unflushed", "P1")


# ──────────────────────────────────────────────────────────────────────────────
# CHECKS — Documentation (P2)
# ──────────────────────────────────────────────────────────────────────────────

@check("IPC-01 : mpmc.rs — doc RING_SIZE correcte (pas 4096)", severity="P2")
def check_mpmc_doc(root: pathlib.Path) -> CheckResult:
    src = read(root, "kernel/src/ipc/channel/mpmc.rs")
    if not src:
        return CheckResult("IPC-01", False, "mpmc.rs introuvable", "P2")

    # La ligne problématique contenait "4096 slots" dans un commentaire doc
    lines_with_4096 = [
        l.strip() for l in src.splitlines()
        if "4096" in l and "slot" in l.lower()
        and not l.strip().startswith("//") is False
    ]
    # Plus précis : chercher une mention active (non-commentée) de 4096 slots
    bad_lines = [
        l for l in src.splitlines()
        if "4096" in l
        and ("slot" in l.lower() or "capacit" in l.lower() or "RING_SIZE" in l)
        and "PATCH" not in l        # ignorer nos propres notes de patch
        and "TODO" not in l
    ]
    if bad_lines:
        return CheckResult("IPC-01", False,
                           f"Mention 4096/slot encore presente:\n  "
                           + "\n  ".join(bad_lines[:3]), "P2")

    return CheckResult("IPC-01", True,
                       "Plus de reference incorrecte '4096 slots' dans mpmc.rs ✓")


@check("TTY-01 : TTY_SEND_TIMEOUT_NS <= 500ms", severity="P2")
def check_tty_timeout(root: pathlib.Path) -> CheckResult:
    src = read(root, "kernel/src/syscall/fs_bridge.rs")
    if not src:
        return CheckResult("TTY-01", False, "fs_bridge.rs introuvable", "P2")

    m = re.search(r'const TTY_SEND_TIMEOUT_NS\s*:\s*u64\s*=\s*([0-9_]+)\s*;', src)
    if not m:
        return CheckResult("TTY-01", False,
                           "TTY_SEND_TIMEOUT_NS non trouve dans fs_bridge.rs", "P2")

    value_str = m.group(1).replace("_", "")
    value     = int(value_str)
    LIMIT_NS  = 500_000_000   # 500 ms

    if value <= LIMIT_NS:
        ms = value // 1_000_000
        return CheckResult("TTY-01", True,
                           f"TTY_SEND_TIMEOUT_NS = {ms} ms (<= 500 ms) ✓")

    ms = value // 1_000_000
    return CheckResult("TTY-01", False,
                       f"TTY_SEND_TIMEOUT_NS = {ms} ms > 500 ms "
                       f"(risque gel shell)", "P2")


@check("FB-01 : CONSOLE UnsafeCell documente (invariant mono-thread)", severity="P2")
def check_console_safety_doc(root: pathlib.Path) -> CheckResult:
    src = read(root, "servers/fb_server/src/main.rs")
    if not src:
        return CheckResult("FB-01", False,
                           "servers/fb_server/src/main.rs introuvable", "P2")

    # Chercher un commentaire SAFETY ou INVARIANT a proximite de CONSOLE/UnsafeCell
    console_pos = src.find("static CONSOLE")
    if console_pos == -1:
        return CheckResult("FB-01", False, "static CONSOLE non trouve", "P2")

    context = src[max(0, console_pos - 400):console_pos + 50]
    keywords = ["SAFETY", "INVARIANT", "mono-thread", "single-thread", "UnsafeCell"]
    found    = [kw for kw in keywords if kw in context]

    if len(found) >= 2:
        return CheckResult("FB-01", True,
                           f"CONSOLE documente ({', '.join(found)}) ✓")
    return CheckResult("FB-01", False,
                       f"CONSOLE UnsafeCell sans documentation SAFETY/INVARIANT adequate "
                       f"(trouve: {found})", "P2")


@check("FB-02 : font path fragile documente (TODO crate exo-font)", severity="P2")
def check_font_path_doc(root: pathlib.Path) -> CheckResult:
    src = read(root, "servers/fb_server/src/main.rs")
    if not src:
        return CheckResult("FB-02", False,
                           "servers/fb_server/src/main.rs introuvable", "P2")

    font_pos = src.find('#[path =')
    if font_pos == -1:
        return CheckResult("FB-02", True,
                           "Pas de #[path] relatif (crate exo-font peut-etre deja utilise) ✓")

    context = src[max(0, font_pos - 300):font_pos + 50]
    if "TODO" in context or "fragile" in context or "exo-font" in context:
        return CheckResult("FB-02", True,
                           "font path fragile documente avec TODO ✓")

    return CheckResult("FB-02", False,
                       "Chemin relatif #[path] sans avertissement TODO fragile", "P2")


# ──────────────────────────────────────────────────────────────────────────────
# RUNNER
# ──────────────────────────────────────────────────────────────────────────────

ALL_CHECKS: List[Callable] = [
    check_exosh_requires_shield,
    check_shield_not_requires_exosh,
    check_canonical_order,
    check_fb_info_guard,
    check_map_page_assert,
    check_mpmc_doc,
    check_tty_timeout,
    check_console_safety_doc,
    check_font_path_doc,
]

SEV_ORDER = {"P0": 0, "P1": 1, "P2": 2}
SEV_COLOR = {"P0": "\033[31m", "P1": "\033[33m", "P2": "\033[36m"}
RESET     = "\033[0m"
GREEN     = "\033[32m"
BOLD      = "\033[1m"


def run_all(root: pathlib.Path, verbose: bool) -> int:
    results: List[CheckResult] = []

    print(f"\n{BOLD}ExoOS — verify_patch_26e1b5ac{RESET}")
    print(f"Repo  : {root}")
    print(f"Checks: {len(ALL_CHECKS)}\n")
    print("─" * 70)

    for fn in ALL_CHECKS:
        r = fn(root)
        # Injecter sévérité depuis décorateur si non définie par le check
        if hasattr(fn, "_check_severity"):
            r.severity = fn._check_severity
        results.append(r)

        sev_str = f"{SEV_COLOR.get(r.severity, '')}{r.severity}{RESET}"
        status  = f"{GREEN}PASS{RESET}" if r.passed else f"\033[31mFAIL{RESET}"

        print(f"[{sev_str}] [{status}]  {r.name}")
        if not r.passed or verbose:
            for line in r.message.splitlines():
                print(f"           {line}")

    print("─" * 70)

    passed  = sum(1 for r in results if r.passed)
    failed  = [r for r in results if not r.passed]
    p0_fail = [r for r in failed if r.severity == "P0"]
    p1_fail = [r for r in failed if r.severity == "P1"]
    p2_fail = [r for r in failed if r.severity == "P2"]

    print(f"\nRésultat : {passed}/{len(results)} checks PASS")
    if failed:
        print(f"Échecs   : {len(p0_fail)} P0  {len(p1_fail)} P1  {len(p2_fail)} P2")
        print("\nÉchecs détaillés :")
        for r in sorted(failed, key=lambda x: SEV_ORDER[x.severity]):
            sev_str = f"{SEV_COLOR.get(r.severity, '')}{r.severity}{RESET}"
            print(f"  [{sev_str}] {r.name}")
            print(f"        → {r.message.splitlines()[0]}")
    else:
        print(f"{GREEN}Tous les checks passent. Prêt pour commit.{RESET}")

    # Exit 1 si au moins un P0 ou P1 échoue
    critical_fail = len(p0_fail) + len(p1_fail)
    return 0 if critical_fail == 0 else 1


def main():
    parser = argparse.ArgumentParser(
        description="Vérifie les patches du commit 26e1b5ac"
    )
    parser.add_argument("--repo",    default=".",
                        help="Racine du dépôt ExoOS (défaut: .)")
    parser.add_argument("--verbose", action="store_true",
                        help="Afficher les messages même pour les checks PASS")
    args = parser.parse_args()

    root = pathlib.Path(args.repo).resolve()
    if not (root / "kernel").is_dir():
        print(f"ERREUR: '{root}' ne semble pas être la racine d'un dépôt ExoOS "
              f"(pas de répertoire 'kernel/')")
        sys.exit(2)

    sys.exit(run_all(root, args.verbose))


if __name__ == "__main__":
    main()
