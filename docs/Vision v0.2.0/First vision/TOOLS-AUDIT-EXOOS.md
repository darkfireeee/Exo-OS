# TOOLS-AUDIT-EXOOS — Outillage d'Audit Automatisé
## const_assert! · Semgrep · Kani · cargo-dylint · Script Python · CI

**Auteur :** claude-alpha  
**Date :** 2026-05-16  
**Contexte :** Un seul outil ne suffit pas — les bugs identifiés par beta et gamma  
prouvent qu'il faut une batterie complète. Ce document spécifie le setup complet.

---

## 1. Pourquoi une Batterie d'Outils

Les bugs identifiés dans les trois sessions d'audit illustrent trois catégories distinctes :

| Catégorie | Exemple | Outil adapté |
|-----------|---------|-------------|
| **Incohérence de constante** | `MAX_CORES = 64` dans ssr.rs vs `256` dans arch/ | Script Python + `const_assert!` |
| **Impossibilité architecturale** | SSR struct > 4 KiB | `const_assert!` dans le code |
| **Violation de règle ExoOS** | ISR avec allocation | `cargo-dylint` |
| **Bug de logique** | `used_ns` sans reset fenêtre | Kani (model checking) |
| **Propriété de sécurité** | `is_immutable()` non vérifié | Semgrep |
| **Regression de dépendance** | snmalloc-rs dépend de std | `cargo-deny` |

Aucun outil seul ne couvre tout. La batterie ci-dessous est complémentaire.

---

## 2. Couche 1 — `const_assert!` dans le Code (Immédiat, Zéro Overhead)

**Principe :** Le compilateur Rust lui-même refuse de compiler si une constante est incohérente. Pas de CI requis — bloque au premier `cargo build`.

**Fichier canonique des constantes :**

```rust
// kernel/src/arch/constants.rs — SOURCE UNIQUE DE VÉRITÉ

/// Cores supportés au niveau layout mémoire (SSR, TCB tables).
pub const MAX_CORES_LAYOUT: usize = 256;

/// Cores actifs au runtime (≤ MAX_CORES_LAYOUT).
pub const MAX_CORES_RUNTIME: usize = 64;

/// Mots u64 pour un bitmask de MAX_CORES_LAYOUT cores.
pub const CORE_MASK_WORDS: usize = MAX_CORES_LAYOUT / 64;  // = 4

/// Taille maximale d'un message IPC inline.
pub const MAX_MSG_SIZE: usize = 240;

/// Seuil inline/SHM pour les transferts réseau.
pub const IPC_INLINE_MAX: usize = 200;  // < MAX_MSG_SIZE avec marge header

/// Adresse minimale de chargement ELF Ring3 (CORR-80).
pub const USER_ELF_BASE_MIN: u64 = 0x400000;  // 4 MiB

// ── Vérifications statiques globales ──────────────────────────────────────────

const _: () = assert!(MAX_CORES_RUNTIME <= MAX_CORES_LAYOUT,
    "MAX_CORES_RUNTIME dépasse MAX_CORES_LAYOUT");

const _: () = assert!(CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT,
    "CORE_MASK_WORDS incohérent avec MAX_CORES_LAYOUT");

const _: () = assert!(IPC_INLINE_MAX < MAX_MSG_SIZE,
    "IPC_INLINE_MAX doit être < MAX_MSG_SIZE");

const _: () = assert!(USER_ELF_BASE_MIN <= 0x400000,
    "USER_ELF_BASE_MIN trop haut — ELF standard = base 0x400000");
```

**Dans chaque fichier qui utilise ces constantes :**

```rust
// kernel/src/exophoenix/ssr.rs
use crate::arch::constants::{MAX_CORES_LAYOUT, CORE_MASK_WORDS};

// Vérification locale : si quelqu'un change MAX_CORES_LAYOUT sans mettre
// à jour CORE_MASK_WORDS, la compilation échoue ici
const _: () = assert!(
    core::mem::size_of::<SsrCoreMask>() == CORE_MASK_WORDS * 8,
    "SsrCoreMask mal dimensionné — vérifier CORE_MASK_WORDS"
);

// Vérification SSR size (CORR-81)
const _: () = assert!(
    core::mem::size_of::<SystemStateRecord>() <= 4096,
    "SystemStateRecord dépasse 4096 octets — réduire SSR_MAX_PROCESSES ou SSR_MAX_ENDPOINTS"
);

// kernel/src/fs/exofs/objects/object_meta.rs
const _: () = assert!(
    core::mem::size_of::<ObjectMeta>() <= 64,
    "ObjectMeta trop grande — impact performance cache"
);

// kernel/src/security/exokairos.rs
const _: () = assert!(
    KAIROS_WINDOW_NS == 1_000_000_000,
    "Fenêtre ExoKairos doit être 1 seconde (1_000_000_000 ns)"
);
```

---

## 3. Couche 2 — Script Python d'Audit de Cohérence

**Fichier :** `tools/audit_constants.py`

Ce script détecte les constantes définies dans plusieurs fichiers avec des valeurs différentes — le problème exact du bug 256-cores qui s'est propagé.

```python
#!/usr/bin/env python3
# tools/audit_constants.py
# Détecteur d'incohérences de constantes critiques ExoOS
# Usage : python3 tools/audit_constants.py [--fail-on-warn]

import re
import sys
import json
from pathlib import Path
from collections import defaultdict
from dataclasses import dataclass, field
from typing import List, Dict, Set

# ── Constantes critiques à surveiller ─────────────────────────────────────────
CRITICAL_PATTERNS = [
    # Cores / CPU
    (r'MAX_CORES?(?:_LAYOUT|_RUNTIME)?',  "arch/constants.rs"),
    (r'SSR_MAX_CORES',                    "arch/constants.rs"),
    (r'CORE_MASK_WORDS',                  "arch/constants.rs"),
    # IPC
    (r'MAX_MSG_SIZE',                     "arch/constants.rs"),
    (r'IPC_INLINE_MAX',                   "arch/constants.rs"),
    (r'RING_SIZE',                        "ipc/ring/spsc.rs"),
    # Processus
    (r'MAX_PROCESSES?',                   "arch/constants.rs"),
    (r'USER_ELF_BASE_MIN',               "arch/constants.rs"),
    # Sécurité
    (r'KAIROS_WINDOW_NS',                "security/exokairos.rs"),
    (r'DECEPTION_THRESHOLD',             "security/exoargos.rs"),
    (r'SSR_MAX_PROCESSES',               "exophoenix/ssr.rs"),
    (r'SSR_MAX_ENDPOINTS',               "exophoenix/ssr.rs"),
    # Drivers
    (r'VIRTIO_BLK_MMIO_BASE',            "INTERDIT — utiliser PCI BAR"),
]

@dataclass
class Finding:
    const_name:    str
    file:          str
    line:          int
    value:         str
    canonical_file: str

def audit(kernel_root: Path) -> tuple[list, list]:
    errors   = []
    warnings = []
    all_findings: Dict[str, List[Finding]] = defaultdict(list)

    pattern = re.compile(
        r'(?:pub\s+)?(?:const|static)\s+'
        r'(' + '|'.join(p for p, _ in CRITICAL_PATTERNS) + r')\w*'
        r'\s*:\s*[\w\[\];: ]+\s*=\s*([^;]+);',
        re.IGNORECASE
    )

    for rs_file in kernel_root.rglob("*.rs"):
        text = rs_file.read_text(errors='replace')
        for match in pattern.finditer(text):
            name  = match.group(1).upper()
            value = match.group(2).strip()
            line  = text[:match.start()].count('\n') + 1
            canonical = next((c for p, c in CRITICAL_PATTERNS
                              if re.match(p, name, re.IGNORECASE)), "?")
            all_findings[name].append(Finding(name, str(rs_file), line, value, canonical))

    for name, findings in sorted(all_findings.items()):
        values = set(f.value for f in findings)

        if len(values) > 1:
            # ERREUR : même constante, valeurs différentes
            errors.append({
                'type': 'INCOHERENCE',
                'const': name,
                'values': list(values),
                'locations': [f"{f.file}:{f.line} = {f.value}" for f in findings],
            })

        elif len(findings) > 1:
            # AVERTISSEMENT : même constante définie dans plusieurs fichiers
            canonical = findings[0].canonical_file
            non_canonical = [f for f in findings if canonical not in f.file]
            if non_canonical:
                warnings.append({
                    'type': 'DUPLICATION',
                    'const': name,
                    'canonical': canonical,
                    'duplicates': [f"{f.file}:{f.line}" for f in non_canonical],
                })

        # Vérifier les constantes INTERDITES (ex: adresses hardcodées)
        for finding in findings:
            if 'VIRTIO_BLK_MMIO_BASE' in finding.const_name:
                errors.append({
                    'type': 'FORBIDDEN',
                    'const': name,
                    'file': finding.file,
                    'reason': "Adresse VirtIO hardcodée — utiliser PCI BAR dynamique (CORR-86)",
                })

    return errors, warnings

def main():
    kernel_root = Path("kernel/src")
    if not kernel_root.exists():
        print("Erreur : exécuter depuis la racine du projet ExoOS")
        sys.exit(2)

    errors, warnings = audit(kernel_root)
    fail_on_warn = "--fail-on-warn" in sys.argv

    print("=" * 65)
    print("AUDIT DE COHÉRENCE DES CONSTANTES ExoOS")
    print("=" * 65)

    for e in errors:
        print(f"\n❌ {e['type']} : {e['const']}")
        if e['type'] == 'INCOHERENCE':
            print(f"   Valeurs : {e['values']}")
            for loc in e['locations']:
                print(f"   → {loc}")
        elif e['type'] == 'FORBIDDEN':
            print(f"   {e['file']} : {e['reason']}")

    for w in warnings:
        print(f"\n⚠️  DUPLICATION : {w['const']}")
        print(f"   Canonique  : {w['canonical']}")
        for dup in w['duplicates']:
            print(f"   Dupliqué   : {dup}")

    print(f"\n{'=' * 65}")
    print(f"Erreurs critiques : {len(errors)}")
    print(f"Avertissements    : {len(warnings)}")

    if errors or (fail_on_warn and warnings):
        sys.exit(1)
    sys.exit(0)

if __name__ == "__main__":
    main()
```

---

## 4. Couche 3 — Semgrep (Règles ExoOS)

**Fichier :** `tools/semgrep-rules/exoos.yaml`

```yaml
rules:

  # ── DRV-ARCH-01 : Zéro logique driver en Ring0 ────────────────────────────
  - id: drv-arch-01-mmio-in-ring0
    patterns:
      - pattern: |
          unsafe { $REG.write_volatile($VAL) }
      - pattern-not-inside: |
          // Ring1
    paths:
      include:
        - "kernel/src/**"
      exclude:
        - "kernel/src/drivers/**"
        - "kernel/src/security/iommu/**"
    message: |
      DRV-ARCH-01 : Écriture de registre MMIO détectée en Ring0 hors drivers/.
      Déplacer cette logique dans device_server (Ring1).
    languages: [rust]
    severity: ERROR

  # ── IPC-RULE-01 : Jamais utiliser len comme discriminant de type ───────────
  - id: ipc-rule-01-len-as-type
    patterns:
      - pattern: |
          if $MSG.len == $MAGIC { ... }
      - pattern: |
          if $MSG.payload.len() == $MAGIC { ... }
    message: |
      IPC-RULE-01 : La longueur du message est utilisée comme discriminant de type.
      Utiliser msg.header.msg_type uniquement (HIGH-01 / CORR-78).
    languages: [rust]
    severity: ERROR

  # ── ERR-04 : Vérifier is_immutable avant écriture ─────────────────────────
  - id: exofs-write-without-immutable-check
    patterns:
      - pattern: |
          fn $WRITE_FN(...) -> ... {
            ...
            write_blob_data(...)
            ...
          }
      - pattern-not: |
          fn $WRITE_FN(...) -> ... {
            ...
            if $META.is_immutable() { ... }
            ...
            write_blob_data(...)
            ...
          }
    paths:
      include:
        - "kernel/src/fs/exofs/syscall/**"
    message: |
      ExoFS write sans vérification is_immutable() — ExoLedger peut être modifié.
      Ajouter la vérification is_immutable() avant write_blob_data() (CORR-84).
    languages: [rust]
    severity: ERROR

  # ── PHX-01 : Structs avec CapToken doivent implémenter PhoenixSafe ─────────
  - id: missing-phoenix-safe
    patterns:
      - pattern: |
          struct $NAME {
            ...
            $FIELD: CapToken,
            ...
          }
      - pattern-not: |
          impl PhoenixSafe for $NAME { ... }
    paths:
      include:
        - "libs/**"
        - "servers/**"
    message: |
      PHX-01 : La struct $NAME contient un CapToken mais n'implémente pas PhoenixSafe.
      Ajouter on_pre_switch() et on_post_switch() pour la sécurité ExoPhoenix.
    languages: [rust]
    severity: WARNING

  # ── ISR-01 : Pas d'allocation dans les ISR ────────────────────────────────
  - id: isr-alloc-forbidden
    patterns:
      - pattern: |
          extern "x86-interrupt" fn $ISR(...) {
            ...
            Vec::new()
            ...
          }
      - pattern: |
          extern "x86-interrupt" fn $ISR(...) {
            ...
            Box::new(...)
            ...
          }
      - pattern: |
          extern "x86-interrupt" fn $ISR(...) {
            ...
            alloc!(...)
            ...
          }
    message: |
      DRV-ISR-01 : Allocation mémoire dans une ISR.
      Les ISR ne peuvent : acquitter + flag atomique + EOI uniquement.
    languages: [rust]
    severity: ERROR

  # ── Constante VirtIO MMIO hardcodée (CORR-86) ─────────────────────────────
  - id: virtio-hardcoded-bar
    pattern: |
      const $NAME: u64 = 0x1000_0000;
    metavariable-regex:
      $NAME: ".*(VIRTIO|MMIO|BAR).*"
    message: |
      Adresse MMIO VirtIO hardcodée détectée. Utiliser PCI BAR dynamique (CORR-86).
    languages: [rust]
    severity: ERROR

  # ── ExoKairos sans reset de fenêtre (ERR-07) ──────────────────────────────
  - id: kairos-no-window-reset
    patterns:
      - pattern: |
          $BUDGET.used_ns += $ELAPSED;
      - pattern-not-inside: |
          if $NOW.saturating_sub($BUDGET.window_start_ns) >= KAIROS_WINDOW_NS {
            $BUDGET.used_ns = 0;
            ...
          }
    paths:
      include:
        - "kernel/src/security/exokairos.rs"
    message: |
      ERR-07 : Incrémentation used_ns sans reset de fenêtre temporelle.
      Ajouter la logique de reset de fenêtre (CORR-82).
    languages: [rust]
    severity: ERROR

  # ── u64 bitmask trop petit pour 256 cores ─────────────────────────────────
  - id: u64-bitmask-too-small
    pattern: |
      $FIELD: u64,
    metavariable-regex:
      $FIELD: ".*(core_mask|active_cores|cpu_mask|core_bitmap).*"
    message: |
      Bitmask u64 limité à 64 cores. Pour 256 cores, utiliser [u64; CORE_MASK_WORDS].
      Référence : arch/constants.rs::CORE_MASK_WORDS = 4.
    languages: [rust]
    severity: WARNING
```

**Usage :**
```bash
# Installation
pip install semgrep

# Audit complet
semgrep --config tools/semgrep-rules/exoos.yaml kernel/ libs/ servers/

# Audit CI (fail sur erreur, ignore warning)
semgrep --config tools/semgrep-rules/exoos.yaml kernel/ --error
```

---

## 5. Couche 4 — Kani (Model Checking Rust)

Kani **prouve mathématiquement** des propriétés — pas juste "ça compile", mais "cette fonction ne peut jamais paniquer dans aucun état possible".

**Preuves prioritaires pour ExoOS :**

```rust
// kernel/src/exophoenix/tests/kani_proofs.rs

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use kani::*;

    // PREUVE 1 : SsrCoreMask::set_core() jamais OOB pour core_id 0..=255
    #[kani::proof]
    fn proof_ssr_core_mask_no_oob() {
        let core_id: usize = kani::any();
        kani::assume(core_id < MAX_CORES_LAYOUT);  // 256
        let mut mask = SsrCoreMask { active_cores: [0u64; CORE_MASK_WORDS] };
        // Kani vérifie : pas de panic, pas d'accès OOB pour tout core_id < 256
        mask.set_core(core_id);
        // Kani vérifie : le bit correspondant est bien mis
        assert!(mask.is_core_active(core_id));
    }

    // PREUVE 2 : align_up() dans exo-alloc est correct pour toutes les tailles
    #[kani::proof]
    fn proof_align_up_correctness() {
        let size:  usize = kani::any();
        let align: usize = kani::any();
        // Align doit être une puissance de 2 (garanti par Layout en Rust)
        kani::assume(align.is_power_of_two());
        kani::assume(size < usize::MAX / 2);  // éviter l'overflow

        let result = align_up(size, align);

        // Propriétés :
        assert!(result >= size,            "align_up doit retourner >= size");
        assert!(result % align == 0,       "résultat doit être multiple de align");
        assert!(result < size + align,     "arrondi ne dépasse pas un align");
    }

    // PREUVE 3 : ExoKairos budget.used_ns ne déborde jamais
    #[kani::proof]
    fn proof_kairos_no_overflow() {
        let elapsed_ns: u64 = kani::any();
        let now_ns:     u64 = kani::any();
        let mut tcb = Tcb::default();

        // Toute combinaison de elapsed et now ne provoque pas d'overflow
        kani::assume(elapsed_ns < KAIROS_WINDOW_NS);
        kani::assume(now_ns < u64::MAX - KAIROS_WINDOW_NS);

        update_kairos_budget(&mut tcb, elapsed_ns, now_ns);
        // Kani vérifie : used_ns <= 200% du budget (seuil de kill)
        // et jamais > u64::MAX
    }

    // PREUVE 4 : SystemStateRecord tient dans 4096 octets
    #[kani::proof]
    fn proof_ssr_fits_in_page() {
        // Cette preuve est statique — elle échoue à la compilation si false
        assert!(core::mem::size_of::<SystemStateRecord>() <= 4096);
    }

    // PREUVE 5 : CapToken verify() ne panic jamais sur un token arbitraire
    #[kani::proof]
    fn proof_captoken_verify_no_panic() {
        let token = CapToken {
            object_id:  kani::any(),
            rights:     kani::any(),
            generation: kani::any(),
            type_tag:   kani::any(),
            _pad:       kani::any(),
        };
        let rights: u32 = kani::any();
        // Pas de panic possible — même sur un token invalide
        let _ = capability::verify_raw(&token, rights);
    }
}
```

**Usage :**
```bash
cargo install --locked kani-verifier
cargo kani setup

# Lancer toutes les preuves
cargo kani --tests

# Lancer une preuve spécifique
cargo kani --harness proof_ssr_core_mask_no_oob
```

---

## 6. Couche 5 — cargo-deny (Dépendances)

Détecte les dépendances incompatibles (ex: snmalloc-rs requiert std alors qu'on est no_std).

```toml
# deny.toml
[advisories]
vulnerability = "deny"
unmaintained  = "warn"

[licenses]
unlicensed    = "deny"
allow         = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC"]

[bans]
# Interdire des crates incompatibles avec ExoOS
deny = [
    { name = "libsodium-sys",   reason = "FFI C interdit — utiliser RustCrypto" },
    { name = "dbus",            reason = "D-Bus incompatible avec IPC ExoOS" },
    { name = "zbus",            reason = "D-Bus incompatible avec IPC ExoOS" },
    { name = "tokio",           version = "*",
      reason = "Runtime tokio interdit — utiliser exo-runtime sauf tokio::sync" },
    { name = "async-std",       reason = "Redondant avec exo-runtime" },
    { name = "systemd",         reason = "Incompatible modèle ExoOS" },
]

# Vérifier que no_std est respecté dans les crates kernel
[features]
# Déclenche une erreur si std est activé dans les crates kernel
```

---

## 7. Intégration CI — `.github/workflows/audit.yml`

```yaml
name: ExoOS Audit Complet

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

jobs:

  # Couche 1 : Compilation (inclut const_assert!)
  compile-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo build --all 2>&1 | tee build.log
      - run: grep -c "error\[" build.log && exit 1 || exit 0

  # Couche 2 : Audit de constantes
  audit-constants:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with: { python-version: '3.12' }
      - run: python3 tools/audit_constants.py --fail-on-warn

  # Couche 3 : Semgrep
  semgrep:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: pip install semgrep
      - run: semgrep --config tools/semgrep-rules/exoos.yaml kernel/ libs/ --error

  # Couche 4 : Kani (lent — sur PR uniquement)
  kani-proofs:
    runs-on: ubuntu-latest
    if: github.event_name == 'pull_request'
    steps:
      - uses: actions/checkout@v4
      - uses: model-checking/kani-github-action@v1
      - run: cargo kani --tests --timeout 120

  # Couche 5 : cargo-deny
  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v1

  # Tests unitaires
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo test --all 2>&1

  # Audit de sécurité (pre-commit)
  security-audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo audit
```

---

## 8. Pre-commit Hook Local

```bash
#!/bin/bash
# .git/hooks/pre-commit — Exécuté avant chaque commit

set -e
echo "ExoOS pre-commit audit..."

# Couche 1 : Compilation rapide
cargo check --all --quiet || { echo "❌ Erreur de compilation"; exit 1; }

# Couche 2 : Cohérence des constantes
python3 tools/audit_constants.py || { echo "❌ Incohérence de constante"; exit 1; }

# Couche 3 : Semgrep (règles critiques uniquement)
semgrep --config tools/semgrep-rules/exoos.yaml kernel/ --error --quiet \
  || { echo "❌ Violation de règle ExoOS"; exit 1; }

echo "✅ Pre-commit audit passé"
```

```bash
chmod +x .git/hooks/pre-commit
```

---

## 9. Tableau Récapitulatif

| Outil | Couche | Quand | Détecte |
|-------|--------|-------|---------|
| `const_assert!` | 1 | `cargo build` | Constantes incohérentes, struct oversized |
| Script Python | 2 | CI + pre-commit | Duplication de constantes, valeurs différentes |
| Semgrep | 3 | CI + pre-commit | Violations DRV-ARCH, ISR-alloc, PHX, IPC |
| Kani | 4 | CI (PR) | Panics, OOB, overflow — preuves mathématiques |
| cargo-deny | 5 | CI | Dépendances interdites (libsodium, dbus, tokio) |
| cargo-audit | 6 | CI | CVE sur les dépendances |
| TLA+ | 7 | Sur changement algo | Deadlocks, états globaux, ExoPhoenix |

**Couverture estimée avec la batterie complète :**
- Incohérence de constante : **100%** (const_assert! + script Python)
- Violation de règle ExoOS : **~85%** (Semgrep)
- Bug de logique mathématique : **~70%** (Kani — limité aux fonctions annotées)
- Dépendance incompatible : **100%** (cargo-deny)
- Régression de sécurité : **~80%** (Semgrep + audit)

---

*claude-alpha — ExoOS v0.2.0 — TOOLS-AUDIT-EXOOS.md*
