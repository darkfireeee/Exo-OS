#!/usr/bin/env python3
"""
verify_all_patches.py — Vérification globale de tous les patches
Exécute les 4 scripts individuels et produit un rapport final.
Usage: python3 tools/verify_all_patches.py [--repo /chemin/vers/Exo-OS]
"""
import sys, os, subprocess

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
SCRIPTS = [
    ("PATCH-P0-PHOENIX",    "verify_p0_phoenix.py"),
    ("PATCH-P0-FORK-STUB",  "verify_p0_fork_stubs.py"),
    ("PATCH-P1-DEBUG",      "verify_p1_exocage.py"),
    ("PATCH-P2-BOOT",       "verify_p2_boot.py"),
]

print("=" * 70)
print("VERIFY ALL PATCHES — ExoOS v0.2.0 Strata — commit 17cf408b")
print("=" * 70)

results = {}
for name, script in SCRIPTS:
    path = os.path.join(SCRIPT_DIR, script)
    print(f"\n{'─' * 70}")
    result = subprocess.run([sys.executable, path], capture_output=False)
    results[name] = result.returncode == 0

print(f"\n{'═' * 70}")
print("RÉCAPITULATIF FINAL")
print(f"{'═' * 70}")
all_pass = True
for name, passed in results.items():
    status = "PASS ✓" if passed else "FAIL ✗"
    print(f"  {status}  {name}")
    if not passed:
        all_pass = False

print(f"\n{'─' * 70}")
if all_pass:
    print("✅ TOUS LES PATCHES VALIDÉS — Prêt pour commit")
else:
    failing = [n for n, p in results.items() if not p]
    print(f"❌ PATCHES EN ÉCHEC: {', '.join(failing)}")
    print("   Relire les fichiers modifiés et re-appliquer les patches.")
print(f"{'─' * 70}")

sys.exit(0 if all_pass else 1)
