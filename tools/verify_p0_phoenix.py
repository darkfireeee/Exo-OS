#!/usr/bin/env python3
"""
verify_p0_phoenix.py — Vérification du PATCH-P0-PHOENIX
Vérifie que la récupération ExoPhoenix fonctionne en production
sans dépendre de TEST_ARMED.
"""
import sys, os, re

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
PATCH_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..'))

def check_file(path, description, checks):
    """Exécute une liste de vérifications sur un fichier."""
    full_path = os.path.join(PATCH_ROOT, path)
    if not os.path.exists(full_path):
        full_path = os.path.join(REPO_ROOT, path)
    if not os.path.exists(full_path):
        print(f"  ✗ Fichier introuvable: {path}")
        return False
    with open(full_path, encoding='utf-8') as f:
        src = f.read()
    ok = True
    for pattern, expected, msg in checks:
        found = bool(re.search(pattern, src, re.MULTILINE))
        status = "✓" if found == expected else "✗"
        if found != expected:
            ok = False
        print(f"  {status} {msg}")
    return ok

print("=" * 60)
print("PATCH-P0-PHOENIX — Vérification resurrection.rs")
print("=" * 60)

path = "kernel/src/exophoenix/resurrection.rs"
checks = [
    # La nouvelle logique phoenix_ready doit être présente
    (r'let phoenix_ready\s*=\s*PHOENIX_STATE\.load\(Ordering::Acquire\)', True,
     "phoenix_ready vérifie PHOENIX_STATE.load()"),
    # test_triggered doit remplacer le swap direct
    (r'let test_triggered\s*=\s*TEST_ARMED\.swap\(false,\s*Ordering::AcqRel\)', True,
     "test_triggered = TEST_ARMED.swap(false, ...)"),
    # La condition combinée doit utiliser les deux variables
    (r'if !phoenix_ready && !test_triggered', True,
     "Condition combinée !phoenix_ready && !test_triggered"),
    # L'ancien guard direct ne doit plus exister (ligne seule avec TEST_ARMED gate)
    (r'^\s+if !TEST_ARMED\.swap\(false,\s*Ordering::AcqRel\)\s*\{\s*\n\s+return false;\s*\n\s+\}',
     False, "Ancien guard TEST_ARMED direct supprimé"),
    # Appel final à recover_kernel_a toujours présent
    (r'recover_kernel_a\(reason,\s*frame\)', True,
     "recover_kernel_a() toujours appelé en sortie"),
    # Commentaire PATCH présent
    (r'PATCH-P0-PHOENIX', True,
     "Commentaire PATCH-P0-PHOENIX présent"),
]

ok = check_file(path, "resurrection.rs", checks)
print()
print("RÉSULTAT:", "PASS ✓" if ok else "FAIL ✗")
sys.exit(0 if ok else 1)
