#!/usr/bin/env python3
"""
check_service_order.py — Validation topologique du service_table Ring1
ExoOS v0.2.0 — Strata

Vérifie :
  1. Cohérence graphe de dépendances (pas de cycle, deps déclarées existent)
  2. Invariant Strata : exo_shield avant exosh
  3. Invariant Strata : exosh ne peut démarrer sans exo_shield
  4. Tous les services critiques sont présents
  5. Résolution topologique (ordre de boot possible)

Usage :
    python3 tools/check_service_order.py [--repo ROOT]
"""

import sys
import re
import pathlib
import argparse
from collections import defaultdict, deque
from typing import Dict, List, Set, Optional, Tuple


def load_service_table(root: pathlib.Path) -> Tuple[Dict, List[str]]:
    """
    Parse service_table.rs et retourne :
      - services : dict nom -> {requires, optional, critical, timeout}
      - order    : liste des noms dans l'ordre de déclaration
    """
    path = root / "servers/init_server/src/service_table.rs"
    if not path.exists():
        print(f"ERREUR: {path} introuvable")
        sys.exit(2)
    src = path.read_text()

    # ─── 1. Extraire les constantes DEPS_* ────────────────────────────────────
    deps_map: Dict[str, List[str]] = {}
    for m in re.finditer(
        r'const (DEPS_\w+)\s*:\s*&\[&str\]\s*=\s*&\[([^\]]*)\]',
        src, re.DOTALL
    ):
        const_name = m.group(1)
        items_raw  = m.group(2)
        # Retirer les lignes de commentaire avant de parser les strings
        items_raw_clean = "\n".join(
            l for l in items_raw.splitlines()
            if not l.strip().startswith("//")
        )
        items = re.findall(r'"([^"]+)"', items_raw_clean)
        deps_map[const_name] = items

    # ─── 2. Extraire les ServiceMetadata dans CANONICAL_SERVICES ─────────────
    services: Dict[str, dict] = {}
    order: List[str] = []

    for block in re.finditer(
        r'ServiceMetadata\s*\{([^}]+)\}', src, re.DOTALL
    ):
        content = block.group(1)

        name_m    = re.search(r'name\s*:\s*"([^"]+)"', content)
        req_m     = re.search(r'requires\s*:\s*(DEPS_\w+|NO_DEPS)', content)
        opt_m     = re.search(r'requires_optional\s*:\s*(DEPS_\w+|NO_DEPS)', content)
        crit_m    = re.search(r'critical\s*:\s*(true|false)', content)
        timeout_m = re.search(r'ready_timeout_ms\s*:\s*([0-9_]+)', content)

        if not name_m:
            continue

        name    = name_m.group(1)
        req_key = req_m.group(1) if req_m else "NO_DEPS"
        opt_key = opt_m.group(1) if opt_m else "NO_DEPS"

        requires = deps_map.get(req_key, [])
        optional = deps_map.get(opt_key, [])
        critical = crit_m.group(1) == "true" if crit_m else False
        timeout  = int(timeout_m.group(1).replace("_", "")) if timeout_m else 0

        services[name] = {
            "requires": requires,
            "optional": optional,
            "critical": critical,
            "timeout_ms": timeout,
        }
        order.append(name)

    return services, order


def topological_sort(services: Dict[str, dict]) -> Optional[List[str]]:
    """
    Kahn's algorithm — retourne l'ordre de démarrage possible,
    ou None s'il y a un cycle.
    """
    in_degree: Dict[str, int] = defaultdict(int)
    adj: Dict[str, List[str]] = defaultdict(list)

    all_names = set(services.keys())

    for svc, meta in services.items():
        for dep in meta["requires"]:
            if dep in all_names:
                adj[dep].append(svc)
                in_degree[svc] += 1
        if svc not in in_degree:
            in_degree[svc] = 0

    queue  = deque(n for n in all_names if in_degree[n] == 0)
    result = []
    while queue:
        node = queue.popleft()
        result.append(node)
        for child in adj[node]:
            in_degree[child] -= 1
            if in_degree[child] == 0:
                queue.append(child)

    if len(result) != len(all_names):
        return None   # cycle détecté
    return result


def run_checks(root: pathlib.Path) -> int:
    OK   = "  ✓"
    FAIL = "  ✗"
    WARN = "  ⚠"

    services, decl_order = load_service_table(root)
    all_errors = 0

    print(f"\n{'─'*65}")
    print("  check_service_order — ExoOS service_table validation")
    print(f"{'─'*65}")
    print(f"  Services déclarés : {len(services)}")
    print(f"  Ordre déclaration : {' → '.join(decl_order)}\n")

    # ── Check 1 : services critiques obligatoires ─────────────────────────────
    REQUIRED = {
        "ipc_router", "memory_server", "vfs_server", "device_server",
        "tty_server", "fb_server", "input_server", "ps2_driver",
        "exosh", "exo_shield", "crypto_server",
    }
    missing = REQUIRED - set(services.keys())
    if missing:
        print(f"{FAIL} CHECK 1 : Services critiques manquants : {sorted(missing)}")
        all_errors += 1
    else:
        print(f"{OK} CHECK 1 : Tous les services critiques présents")

    # ── Check 2 : toutes les dépendances existent ─────────────────────────────
    dangling = []
    for svc, meta in services.items():
        for dep in meta["requires"]:
            if dep not in services:
                dangling.append((svc, dep))
    if dangling:
        print(f"{FAIL} CHECK 2 : Dépendances manquantes :")
        for svc, dep in dangling:
            print(f"         {svc!r} requires {dep!r} (non déclaré)")
        all_errors += len(dangling)
    else:
        print(f"{OK} CHECK 2 : Toutes les dépendances existent")

    # ── Check 3 : pas de cycle ────────────────────────────────────────────────
    boot_order = topological_sort(services)
    if boot_order is None:
        print(f"{FAIL} CHECK 3 : CYCLE DÉTECTÉ dans le graphe de dépendances !")
        all_errors += 1
    else:
        print(f"{OK} CHECK 3 : Graphe acyclique — ordre valide trouvé")
        print(f"         Boot order : {' → '.join(boot_order)}")

    # ── Check 4 [P0] : exo_shield dans DEPS_EXOSH ────────────────────────────
    if "exo_shield" in services.get("exosh", {}).get("requires", []):
        print(f"{OK} CHECK 4 : exosh requires exo_shield [STRATA-SEC-01]")
    else:
        print(f"{FAIL} CHECK 4 [P0] : exosh NE requiert PAS exo_shield !")
        print(f"         exosh.requires = {services.get('exosh', {}).get('requires')}")
        all_errors += 1

    # ── Check 5 [P0] : exo_shield ne dépend pas d'exosh ─────────────────────
    if "exosh" not in services.get("exo_shield", {}).get("requires", []):
        print(f"{OK} CHECK 5 : exo_shield ne dépend pas d'exosh [STRATA-SEC-01]")
    else:
        print(f"{FAIL} CHECK 5 [P0] : exo_shield REQUIERT exosh (inversion !)") 
        all_errors += 1

    # ── Check 6 [P0] : exo_shield avant exosh dans l'ordre topologique ───────
    if boot_order:
        try:
            idx_shield = boot_order.index("exo_shield")
            idx_exosh  = boot_order.index("exosh")
            if idx_shield < idx_exosh:
                print(f"{OK} CHECK 6 : exo_shield (pos {idx_shield}) avant "
                      f"exosh (pos {idx_exosh}) dans boot order")
            else:
                print(f"{FAIL} CHECK 6 [P0] : exosh (pos {idx_exosh}) avant "
                      f"exo_shield (pos {idx_shield}) !")
                all_errors += 1
        except ValueError as e:
            print(f"{WARN} CHECK 6 : {e}")

    # ── Check 7 : timeout raisonnables ────────────────────────────────────────
    bad_timeouts = [
        (svc, m["timeout_ms"])
        for svc, m in services.items()
        if m["timeout_ms"] == 0
    ]
    if bad_timeouts:
        print(f"{WARN} CHECK 7 : Timeouts à zéro (vérifier) : "
              f"{[s for s,_ in bad_timeouts]}")
    else:
        print(f"{OK} CHECK 7 : Tous les timeouts > 0")

    # ── Résumé ────────────────────────────────────────────────────────────────
    print(f"\n{'─'*65}")
    if all_errors == 0:
        print("  RÉSULTAT : ✓ Tous les checks passent")
    else:
        print(f"  RÉSULTAT : ✗ {all_errors} erreur(s) détectée(s)")
    print(f"{'─'*65}\n")

    return 0 if all_errors == 0 else 1


def main():
    parser = argparse.ArgumentParser(
        description="Validation topologique du service_table ExoOS"
    )
    parser.add_argument("--repo", default=".",
                        help="Racine du dépôt ExoOS")
    args = parser.parse_args()

    root = pathlib.Path(args.repo).resolve()
    if not (root / "servers" / "init_server").is_dir():
        print(f"ERREUR : '{root}' ne contient pas servers/init_server/")
        sys.exit(2)

    sys.exit(run_checks(root))


if __name__ == "__main__":
    main()
