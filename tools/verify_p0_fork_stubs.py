#!/usr/bin/env python3
"""
verify_p0_fork_stubs.py — Vérification du PATCH-P0-FORK-STUB
Vérifie que les stubs morts fork/execve dans table.rs sont documentés,
et que dispatch.rs route correctement avant la table.
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
print("PATCH-P0-FORK-STUB — Vérification table.rs + dispatch.rs")
print("=" * 60)

ok = True

# --- Vérification table.rs ---
print("\n[1] table.rs — stubs documentés")
src_table = load("kernel/src/syscall/table.rs")
table_checks = [
    ("STRATA-DISPATCH-01",         True,  "Tag STRATA-DISPATCH-01 présent"),
    ("code mort",                  True,  "Mention 'code mort' dans les commentaires"),
    ("handle_fork_like_inplace",   True,  "Référence à handle_fork_like_inplace"),
    ("handle_execve_inplace",      True,  "Référence à handle_execve_inplace"),
    ("PATCH-P0-FORK-STUB",         True,  "Tag PATCH-P0-FORK-STUB présent"),
]
for pattern, expected, msg in table_checks:
    found = pattern in src_table
    status = "✓" if found == expected else "✗"
    if found != expected:
        ok = False
    print(f"  {status} {msg}")

# --- Vérification dispatch.rs (dans le repo original) ---
print("\n[2] dispatch.rs — routing actif (dans repo)")
dispatch_path = os.path.join(REPO_ROOT, "kernel/src/syscall/dispatch.rs")
if os.path.exists(dispatch_path):
    with open(dispatch_path, encoding='utf-8') as f:
        src_dispatch = f.read()
    dispatch_checks = [
        (r'effective_nr == .*SYS_FORK',    True, "SYS_FORK intercepté avant table"),
        (r'effective_nr == .*SYS_VFORK',   True, "SYS_VFORK intercepté avant table"),
        (r'effective_nr == .*SYS_EXECVE',  True, "SYS_EXECVE intercepté avant table"),
        ("handle_fork_like_inplace",       True, "handle_fork_like_inplace appelé"),
        ("handle_execve_inplace",          True, "handle_execve_inplace appelé"),
    ]
    for pattern, expected, msg in dispatch_checks:
        found = bool(re.search(pattern, src_dispatch)) if '.*' in pattern else (pattern in src_dispatch)
        status = "✓" if found == expected else "✗"
        if found != expected:
            ok = False
        print(f"  {status} {msg}")
else:
    print("  ⚠ dispatch.rs non trouvé dans le repo (vérification skippée)")

# --- Vérification fork.rs implémenté ---
print("\n[3] fork.rs — do_fork implémenté (dans repo)")
fork_path = os.path.join(REPO_ROOT, "kernel/src/process/lifecycle/fork.rs")
if os.path.exists(fork_path):
    with open(fork_path, encoding='utf-8') as f:
        src_fork = f.read()
    fork_checks = [
        (r'pub fn do_fork',             True,  "do_fork() défini"),
        (r'PID_ALLOCATOR\.alloc\(\)',  True,  "PID_ALLOCATOR.alloc() appelé"),
        (r'AddressSpaceCloner',         True,  "AddressSpaceCloner utilisé (CoW)"),
        ("ForkResult",                  True,  "ForkResult retourné"),
        ("ENOSYS",                      False, "Pas de ENOSYS dans do_fork"),
    ]
    for pattern, expected, msg in fork_checks:
        found = bool(re.search(pattern, src_fork)) if '\\' in pattern or '(' in pattern else (pattern in src_fork)
        status = "✓" if found == expected else "✗"
        if found != expected:
            ok = False
        print(f"  {status} {msg}")
else:
    print("  ⚠ fork.rs non trouvé (vérification skippée)")

print()
print("RÉSULTAT:", "PASS ✓" if ok else "FAIL ✗")
sys.exit(0 if ok else 1)
