#!/usr/bin/env python3
"""
check_ipc_policy_mirror.py — Vérifie que le DAG ExoCordon du routeur Ring 1
est le miroir EXACT de la politique IPC du kernel.

Sources comparées :
  - kernel/src/security/ipc_policy.rs  : static POLICY (paires ServiceClass)
  - servers/ipc_router/src/exocordon.rs : static AUTHORIZED_GRAPH (AuthEdge)

Règle IPC-01 / FIX-EXOCORDON-02 : même ensemble de paires orientées, sinon le
routeur (IpcBroker, wildcard kernel) blanchit des chemins refusés en direct ou
bloque des chemins légitimes.

Usage : python3 tools/check_ipc_policy_mirror.py [--repo ROOT]
Retourne 0 si miroir exact, 1 sinon.
"""

import re
import sys
import pathlib
import argparse

# Mapping ServiceClass (kernel) -> ServiceId (exocordon)
CLASS_TO_ID = {
    "InitServer": "Init",
    "IpcBroker": "IpcBroker",
    "MemoryServer": "Memory",
    "VfsServer": "Vfs",
    "CryptoServer": "Crypto",
    "DeviceServer": "Device",
    "NetworkServer": "Network",
    "SchedulerServer": "Scheduler",
    "InputServer": "Input",
    "TtyServer": "Tty",
    "FbServer": "Fb",
    "Ps2Driver": "Ps2",
    "VirtioDriver": "VirtioDrivers",
    "ExoShield": "ExoShield",
    "Exosh": "Exosh",
}

KERNEL_PAIR = re.compile(
    r"\(ServiceClass::(\w+),\s*ServiceClass::(\w+)\)"
)
EDGE = re.compile(
    r"AuthEdge::new\(ServiceId::(\w+),\s*ServiceId::(\w+)"
)


def extract_kernel_pairs(text: str):
    # Limiter à la définition de POLICY pour ne pas attraper les tests
    m = re.search(r"static POLICY[^=]*=\s*&\[(.*?)\];", text, re.DOTALL)
    if not m:
        return None
    return [(a, b) for a, b in KERNEL_PAIR.findall(m.group(1))]


def extract_router_edges(text: str):
    m = re.search(r"static AUTHORIZED_GRAPH[^=]*=\s*\[(.*?)\n\];", text, re.DOTALL)
    if not m:
        return None
    return [(a, b) for a, b in EDGE.findall(m.group(1))]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = pathlib.Path(args.repo).resolve()

    kernel_file = repo / "kernel/src/security/ipc_policy.rs"
    router_file = repo / "servers/ipc_router/src/exocordon.rs"

    kernel_pairs = extract_kernel_pairs(kernel_file.read_text(encoding="utf-8"))
    router_edges = extract_router_edges(router_file.read_text(encoding="utf-8"))

    if kernel_pairs is None:
        print("ERREUR: static POLICY introuvable dans ipc_policy.rs")
        return 1
    if router_edges is None:
        print("ERREUR: static AUTHORIZED_GRAPH introuvable dans exocordon.rs")
        return 1

    kernel_set = set()
    for a, b in kernel_pairs:
        if a not in CLASS_TO_ID or b not in CLASS_TO_ID:
            print(f"ERREUR: ServiceClass inconnu dans le mapping: {a} ou {b}")
            return 1
        kernel_set.add((CLASS_TO_ID[a], CLASS_TO_ID[b]))
    router_set = set(router_edges)

    print(f"kernel POLICY        : {len(kernel_pairs)} paires ({len(kernel_set)} uniques)")
    print(f"router AUTHORIZED_GRAPH : {len(router_edges)} arêtes ({len(router_set)} uniques)")

    missing = kernel_set - router_set     # dans kernel, absent routeur
    extra = router_set - kernel_set       # dans routeur, absent kernel

    ok = True
    if len(kernel_pairs) != len(kernel_set):
        print("✗ Doublons dans POLICY kernel")
        ok = False
    if len(router_edges) != len(router_set):
        print("✗ Doublons dans AUTHORIZED_GRAPH routeur")
        ok = False
    if missing:
        ok = False
        print(f"✗ {len(missing)} paires kernel ABSENTES du routeur (chemins bloqués) :")
        for a, b in sorted(missing):
            print(f"    {a} -> {b}")
    if extra:
        ok = False
        print(f"✗ {len(extra)} arêtes routeur HORS politique kernel (blanchiment possible) :")
        for a, b in sorted(extra):
            print(f"    {a} -> {b}")

    if ok:
        print("✓ Miroir exact — ExoCordon == ipc_policy.rs")
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())
