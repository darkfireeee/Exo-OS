# ExoOS v0.2.0 — Audit Outillage (BLOC 0)
## 13 Critères d'Outillage : 0/13 Présents

**Auteur :** claude-beta  
**Date :** 2026-05-20  
**Sévérité :** P1 — Le MASTER-CHECKLIST exige 13/13 avant de passer au BLOC 1  
**Checklist :** BLOC 0, O-01 à O-13

---

## Constat global

Le BLOC 0 (Outillage d'Audit) est **entièrement absent**. Aucun des 13
critères n'est satisfait. La checklist indique que le seuil BLOC 0 → BLOC 1
est `13/13`. Aucun travail de v0.2.0 ne peut être validé sans cet outillage.

```
Critères BLOC 0 présents : 0 / 13
Seuil de passage        : 13 / 13
```

---

## O-01 — arch/constants.rs inexistant

**Requis :** `kernel/src/arch/constants.rs`

Ce fichier doit centraliser toutes les constantes architecturales
(adresses MMIO, limites CPU, valeurs MSR) actuellement dispersées dans
plusieurs modules.

**État :** `find kernel/src/arch -name "constants.rs"` → **aucun résultat**

**À créer :**

```rust
// kernel/src/arch/constants.rs
//
// Constantes architecturales canoniques — source unique de vérité.
// Toute constante dupliquée dans un autre fichier doit référencer ici.

/// Adresse MMIO LAPIC par défaut (remplacée après lecture ACPI MADT)
pub const DEFAULT_LAPIC_BASE: u64 = 0xFEE0_0000;

/// Adresse MMIO I/O APIC par défaut
pub const DEFAULT_IOAPIC_BASE: u64 = 0xFEC0_0000;

/// Nombre maximum de CPUs (cohérent avec memory/core/constants.rs::MAX_CPUS)
pub const ARCH_MAX_CPUS: usize = 256;

/// Limite physique adressable x86_64 (46 bits = 64 TiB)
pub const PHYS_ADDR_BITS: u32 = 46;
pub const PHYS_ADDR_MAX: u64 = (1u64 << PHYS_ADDR_BITS) - 1;

// ... autres constantes arch ...
```

---

## O-02 — const_assert! SSR manquant dans ssr.rs

**Requis :** `const_assert!(SSR_SIZE <= 4096)` dans `kernel/src/exophoenix/ssr.rs`  
**Checklist :** O-02, P-01

**État :** `kernel/src/exophoenix/ssr.rs` n'a que des `debug_assert!`
sur les offsets individuels. Aucun `const_assert!` sur `SSR_SIZE <= 4096`.

**À ajouter dans ssr.rs :**

```rust
// kernel/src/exophoenix/ssr.rs

// VÉRIFICATION STATIQUE P-01 : la SSR doit tenir dans une page (4096 octets)
// pour être mappable atomiquement entre Kernel A et Kernel B.
const _: () = assert!(
    SSR_SIZE <= 4096,
    "SSR_SIZE dépasse 4096 octets — ne peut pas être mappé en une seule page"
);
```

---

## O-03 à O-05 — const_assert! manquants dans exokairos.rs et physmap.rs

**Requis :**
- `O-03` : `const_assert!(KAIROS_WINDOW_NS ...)` dans `security/exokairos.rs`
- `O-04` : `const_assert!(PHYSMAP_INITIAL_COVERAGE ...)` dans `memory/physmap.rs`
- `O-05` : `const_assert!(CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT)`

**État :**
- `KAIROS_WINDOW_NS` n'existe pas (ExoKairos n'a pas de fenêtre temporelle — voir SEC-02)
- `PHYSMAP_INITIAL_COVERAGE` n'est pas définie comme constante nommée
- `CORE_MASK_WORDS` n'est pas visible dans les sources kernel (délégué à crate externe `exo-phoenix-ssr`)

**Action :**

```rust
// kernel/src/security/exokairos.rs
pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000; // 1 seconde
const _: () = assert!(KAIROS_WINDOW_NS >= 100_000_000, "fenêtre trop courte");
const _: () = assert!(KAIROS_WINDOW_NS <= 60_000_000_000, "fenêtre trop longue");

// kernel/src/memory/virtual/physmap.rs (ou constants.rs)
pub const PHYSMAP_INITIAL_COVERAGE: usize = 1 * 1024 * 1024 * 1024; // 1 GiB initial
const _: () = assert!(PHYSMAP_INITIAL_COVERAGE >= 1024 * 1024 * 1024, "< 1 GiB");
```

---

## O-06 à O-07 — audit_constants.py inexistant

**Requis :** `tools/audit_constants.py`  
**État :** `find . -name "audit_constants.py"` → **aucun résultat**

Ce script doit parser les fichiers `README.md` des sous-modules pour extraire
les tableaux de constantes et les comparer avec les valeurs dans les fichiers
`.rs` correspondants. Les 4 divergences de `AUDIT-IPC-DOC.md` auraient été
détectées automatiquement.

**Squelette minimal :**

```python
#!/usr/bin/env python3
# tools/audit_constants.py
#
# Vérifie la cohérence des constantes entre docs/ et kernel/src/

import re, sys
from pathlib import Path

KERNEL_ROOT = Path("kernel/src")
DOCS_ROOT   = Path("docs")

# Patterns extraits des README.md
DOC_CONST_PATTERN = re.compile(
    r'\|\s*`([A-Z_]+)`\s*\|\s*([\d\s]+(?:ms|ns|KiB|MiB|GiB)?)\s*\|'
)

# Patterns extraits des fichiers .rs  
RS_CONST_PATTERN = re.compile(
    r'pub const ([A-Z_]+):\s*\w+\s*=\s*([\d_]+)'
)

# ... logique de comparaison ...

if __name__ == "__main__":
    errors = audit_all_constants()
    sys.exit(0 if not errors else 1)
```

---

## O-08 à O-09 — semgrep-rules/exoos.yaml inexistant

**Requis :** `tools/semgrep-rules/exoos.yaml`  
**État :** `find . -name "*.yaml" -path "*semgrep*"` → **aucun résultat**

Règles minimales à implémenter :

```yaml
# tools/semgrep-rules/exoos.yaml

rules:
  - id: exoos-no-alloc-in-isr
    pattern: |
      fn $FUNC(...) { ... alloc!(...) ... }
    message: "Allocation interdite dans les ISR (RÈGLE NOALLOC-ISR)"
    languages: [rust]
    severity: ERROR

  - id: exoos-no-panic-in-isr
    pattern: |
      fn $FUNC(...) { ... panic!(...) ... }
    message: "panic! interdit dans les ISR"
    languages: [rust]
    severity: ERROR

  - id: exoos-unsafe-without-safety-comment
    pattern: |
      unsafe { ... }
    message: "bloc unsafe sans commentaire // SAFETY:"
    fix: "Ajouter // SAFETY: <justification> avant le bloc"
    languages: [rust]
    severity: WARNING

  - id: exoos-no-tokio-runtime
    pattern: tokio::runtime::Runtime::new()
    message: "tokio-runtime interdit dans ExoOS"
    languages: [rust]
    severity: ERROR
```

---

## O-10 à O-11 — deny.toml inexistant

**Requis :** `deny.toml` à la racine du workspace  
**État :** `find . -name "deny.toml"` → **aucun résultat**

```toml
# deny.toml

[bans]
deny = [
  # Interdit par LIBS-REJECTION-LOG.md
  { name = "libsodium-sys" },
  { name = "dbus" },
  { name = "zbus" },
  { name = "tokio" },      # sauf features async-std compat si nécessaire
  { name = "std" },        # le kernel est no_std
]

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC"]
deny  = ["GPL-2.0", "GPL-3.0", "LGPL-2.0", "LGPL-3.0"]  # incompatibles kernel

[advisories]
db-path   = "~/.cargo/advisory-db"
db-urls   = ["https://github.com/rustsec/advisory-db"]
vulnerability = "deny"
unmaintained  = "warn"
```

---

## O-12 — pre-commit hook inexistant

**Requis :** `.git/hooks/pre-commit` (ou via `pre-commit` framework)  
**État :** absent

```bash
#!/bin/bash
# .git/hooks/pre-commit

set -e

echo "=== ExoOS pre-commit checks ==="

# 1. cargo fmt check
cargo fmt --all -- --check || { echo "FAIL: fmt"; exit 1; }

# 2. cargo clippy
cargo clippy --all --no-default-features -- -D warnings || { echo "FAIL: clippy"; exit 1; }

# 3. audit_constants.py
python3 tools/audit_constants.py || { echo "FAIL: constantes désynchronisées"; exit 1; }

# 4. cargo deny
cargo deny check || { echo "FAIL: deny"; exit 1; }

# 5. semgrep (si installé)
if command -v semgrep &>/dev/null; then
    semgrep --config tools/semgrep-rules/exoos.yaml kernel/src/ \
        --error || { echo "FAIL: semgrep"; exit 1; }
fi

echo "=== pre-commit: OK ==="
```

---

## O-13 — .github/workflows/audit.yml inexistant

**Requis :** workflow CI qui exécute les mêmes checks que le pre-commit  
**État :** `find . -name "audit.yml"` → **aucun résultat**

```yaml
# .github/workflows/audit.yml

name: ExoOS Audit

on: [push, pull_request]

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - name: cargo fmt
        run: cargo fmt --all -- --check

      - name: cargo clippy
        run: cargo clippy --all --no-default-features -- -D warnings

      - name: audit_constants
        run: python3 tools/audit_constants.py

      - name: cargo deny
        uses: EmbarkStudios/cargo-deny-action@v1

      - name: semgrep
        uses: semgrep/semgrep-action@v1
        with:
          config: tools/semgrep-rules/exoos.yaml
```

---

## Plan de réalisation BLOC 0

Ordre recommandé (chaque item débloque le suivant) :

```
Semaine 1 :
  [1] Créer arch/constants.rs (O-01)
  [2] Ajouter const_assert! SSR (O-02), exokairos (O-03), physmap (O-04), core_mask (O-05)
  [3] Créer tools/audit_constants.py (O-06) + valider 0 erreurs (O-07)

Semaine 2 :
  [4] Créer deny.toml (O-10) + cargo deny check 0 violations (O-11)
  [5] Créer tools/semgrep-rules/exoos.yaml (O-08) + semgrep 0 violations (O-09)
  [6] Installer pre-commit hook (O-12)
  [7] Créer .github/workflows/audit.yml (O-13)
```

---

*claude-beta — ExoOS v0.2.0 Audit — AUDIT-TOOLING.md*
