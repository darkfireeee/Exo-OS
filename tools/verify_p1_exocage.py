#!/usr/bin/env python3
"""
verify_p1_exocage.py — Vérification du PATCH-P1-DEBUG
Vérifie que les debug_assert! sur les bornes TCB ont été promus en assert!
"""
import sys, os, re

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
PATCH_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..'))

def load(path):
    for root in [PATCH_ROOT, REPO_ROOT]:
        p = os.path.join(root, path)
        if os.path.exists(p):
            with open(p, encoding='utf-8') as f:
                return f.read()
    raise FileNotFoundError(path)

print("=" * 60)
print("PATCH-P1-DEBUG — Vérification exocage.rs")
print("=" * 60)

src = load("kernel/src/security/exocage.rs")
ok = True

# Vérifier que les 4 debug_assert! TCB bounds ont été promus
checks = [
    # Aucun debug_assert sur TCB bounds ne doit rester
    ("debug_assert!(offset + 8 <= 88",   False,
     "debug_assert! offset+8 ≤ 88 supprimé (write u64)"),
    ("debug_assert!(offset < 88",         False,
     "debug_assert! offset < 88 supprimé (write/read u8)"),
    # Les assert! correspondants doivent exister
    (r'assert!\(offset \+ 8 <= 88',       True,
     "assert! offset+8 ≤ 88 présent (release-visible)"),
    (r'assert!\(offset < 88',             True,
     "assert! offset < 88 présent (release-visible)"),
    # Le commentaire de patch
    ("PATCH-P1-DEBUG",                    True,
     "Commentaire PATCH-P1-DEBUG présent"),
    # Le debug_assert threat_score doit encore exister (protégé par .min(100))
    ('debug_assert!(score <= 100',        True,
     "debug_assert! threat_score conservé (clamp .min(100) présent)"),
]

for pattern, expected, msg in checks:
    if expected:
        found = bool(re.search(pattern, src)) if pattern.startswith('r\'') or '\\' in pattern else (pattern in src)
    else:
        found = bool(re.search(pattern, src)) if pattern.startswith('r\'') or '\\' in pattern else (pattern in src)
    status = "✓" if found == expected else "✗"
    if found != expected:
        ok = False
    print(f"  {status} {msg}")

# Compter les assert! pour bornes TCB (doit être exactement 4)
count_assert = len(re.findall(r'assert!\(offset', src))
count_debug = len(re.findall(r'debug_assert!\(offset', src))
print(f"  {'✓' if count_assert == 4 else '✗'} Exactement 4 assert! offset présents (trouvé: {count_assert})")
print(f"  {'✓' if count_debug == 0 else '✗'} Zéro debug_assert! offset restant (trouvé: {count_debug})")
if count_assert != 4 or count_debug != 0:
    ok = False

print()
print("RÉSULTAT:", "PASS ✓" if ok else "FAIL ✗")
sys.exit(0 if ok else 1)
