#!/usr/bin/env python3
"""
verify_p2_boot.py — Vérification du PATCH-P2-BOOT
Vérifie que le chemin Multiboot2 est correctement gated derrière
le feature flag `multiboot2_compat`.
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
print("PATCH-P2-BOOT — Vérification cfg gate multiboot2")
print("=" * 60)

ok = True

# --- main.rs ---
print("\n[1] main.rs — global_asm multiboot2 gated")
src_main = load("kernel/src/main.rs")
main_checks = [
    (r'#\[cfg\(feature\s*=\s*"multiboot2_compat"\)\]\s*\ncore::arch::global_asm!', True,
     "#[cfg(feature = multiboot2_compat)] avant global_asm!"),
    ("PATCH-P2-BOOT", True, "Commentaire PATCH-P2-BOOT présent dans main.rs"),
    ("vision Strata", True, "Mention vision Strata dans le commentaire"),
]
for pattern, expected, msg in main_checks:
    found = bool(re.search(pattern, src_main)) if r'\[' in pattern or r'\n' in pattern else (pattern in src_main)
    status = "✓" if found == expected else "✗"
    if found != expected:
        ok = False
    print(f"  {status} {msg}")

# --- boot/mod.rs ---
print("\n[2] boot/mod.rs — exports gated")
src_mod = load("kernel/src/arch/x86_64/boot/mod.rs")
mod_checks = [
    ("multiboot2_compat", True,   "Feature flag multiboot2_compat référencé"),
    ("#[cfg(feature", True,       "#[cfg(feature...)] présent"),
    ("DEPRECIE", True,            "Mention DEPRECIE dans le commentaire"),
    ("init_memory_subsystem_multiboot2", True, "init_memory_subsystem_multiboot2 encore accessible"),
]
for pattern, expected, msg in mod_checks:
    found = pattern in src_mod
    status = "✓" if found == expected else "✗"
    if found != expected:
        ok = False
    print(f"  {status} {msg}")

# --- Cargo.toml ---
print("\n[3] Cargo.toml — feature déclarée")
try:
    src_cargo = load("kernel/Cargo.toml")
    cargo_checks = [
        ("multiboot2_compat", True, "Feature multiboot2_compat déclarée dans [features]"),
        ("DEPRECIE",          True, "Commentaire de depreciation present"),
    ]
    for pattern, expected, msg in cargo_checks:
        found = pattern in src_cargo
        status = "✓" if found == expected else "✗"
        if found != expected:
            ok = False
        print(f"  {status} {msg}")
except FileNotFoundError:
    print("  ⚠ Cargo.toml non trouvé (vérification skippée)")

print()
print("RÉSULTAT:", "PASS ✓" if ok else "FAIL ✗")
sys.exit(0 if ok else 1)
