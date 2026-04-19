# CORR-55 — forge.rs : Hashes image nuls (CRIT-01)

**Sévérité :** 🔴 CRITIQUE — BLOQUANT  
**Fichier :** `kernel/src/exophoenix/forge.rs`  
**Impact :** `reconstruct_kernel_a()` retourne systématiquement `Err(MerkleVerifyFailed)` → `PhoenixState::Degraded` permanent

---

## Problème

```rust
// ÉTAT ACTUEL — lignes 38 et 42
static A_IMAGE_HASH: [u8; 32] = [0u8; 32]; // [ADAPT] hash réel ici
static A_MERKLE_ROOT: [u8; 32] = [0u8; 32]; // [ADAPT] hash réel ici
```

`reconstruct_kernel_a()` calcule le hash BLAKE3 de `.text` + `.rodata` de Kernel A et le compare à `A_MERKLE_ROOT`. Avec 32 zéros comme racine de référence, **aucun hash légitime ne correspondra jamais** — la reconstruction est structurellement impossible.

---

## Correction

Ces constantes ne peuvent pas être hardcodées statiquement dans le code source car elles dépendent du binaire compilé. Il faut un **mécanisme de provisionnement au build-time**.

### Étape 1 — Définir les constantes via `build.rs`

```rust
// kernel/build.rs — ajout
use std::process::Command;

fn main() {
    // ... lignes existantes ...

    // Calculer le hash BLAKE3 du binaire Kernel A (après linkage)
    // Note: ce hash est calculé sur l'image ELF finale par un script post-build.
    // Pour l'instant, utiliser un mécanisme d'injection via variable d'environnement.
    let a_hash_hex = std::env::var("EXO_KERNEL_A_HASH")
        .unwrap_or_else(|_| "0".repeat(64)); // fallback dev uniquement
    let a_root_hex = std::env::var("EXO_KERNEL_A_MERKLE_ROOT")
        .unwrap_or_else(|_| "0".repeat(64));

    println!("cargo:rustc-env=EXO_KERNEL_A_HASH={a_hash_hex}");
    println!("cargo:rustc-env=EXO_KERNEL_A_MERKLE_ROOT={a_root_hex}");
    println!("cargo:rerun-if-env-changed=EXO_KERNEL_A_HASH");
    println!("cargo:rerun-if-env-changed=EXO_KERNEL_A_MERKLE_ROOT");
}
```

### Étape 2 — Remplacer les statics par des constantes compilées

```rust
// kernel/src/exophoenix/forge.rs — lignes 35–45

/// Hash BLAKE3 de l'image Kernel A (.text ++ .rodata), injecté au build-time.
/// Provisionnement : EXO_KERNEL_A_HASH=<64 hex chars> cargo build
static A_IMAGE_HASH: [u8; 32] = {
    const HEX: &str = env!("EXO_KERNEL_A_HASH");
    hex_to_bytes_32(HEX)
};

/// Racine Merkle de l'image Kernel A, injectée au build-time.
/// Provisionnement : EXO_KERNEL_A_MERKLE_ROOT=<64 hex chars> cargo build
static A_MERKLE_ROOT: [u8; 32] = {
    const HEX: &str = env!("EXO_KERNEL_A_MERKLE_ROOT");
    hex_to_bytes_32(HEX)
};

/// Convertit un string hex 64 chars en [u8; 32] à la compilation.
const fn hex_to_bytes_32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() == 64, "EXO_KERNEL_A_HASH must be 64 hex chars");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        let hi = hex_nibble(b[i * 2]);
        let lo = hex_nibble(b[i * 2 + 1]);
        out[i] = (hi << 4) | lo;
        i += 1;
    }
    out
}

const fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("invalid hex character in EXO_KERNEL_A_HASH"),
    }
}
```

### Étape 3 — Guard de compilation en mode production

```rust
// À ajouter dans reconstruct_kernel_a(), avant toute vérification :
#[cfg(not(feature = "exo-dev"))]
if A_MERKLE_ROOT == [0u8; 32] {
    // En production, refuser de démarrer avec des hashes nuls.
    return Err(ForgeError::HashNotProvisioned);
}
```

### Étape 4 — Script de provisionnement post-build

```bash
#!/usr/bin/env bash
# scripts/provision_kernel_hashes.sh
# À exécuter après `cargo build` pour calculer les vrais hashes.
KERNEL_A_ELF="target/x86_64-exo-none/release/exo-kernel-a"

# Hash BLAKE3 de la section .text + .rodata
A_HASH=$(b3sum --no-names \
    <(objcopy --dump-section .text=/dev/stdout "$KERNEL_A_ELF") \
    <(objcopy --dump-section .rodata=/dev/stdout "$KERNEL_A_ELF") \
    2>/dev/null | awk '{print $1}')

echo "export EXO_KERNEL_A_HASH=$A_HASH"
echo "export EXO_KERNEL_A_MERKLE_ROOT=$A_HASH"  # racine simplifiée pour Phase1
```

---

## Ajouter à `ForgeError`

```rust
// Dans l'enum ForgeError :
/// Les hashes de l'image Kernel A n'ont pas été provisionnés au build-time.
/// Injecter EXO_KERNEL_A_HASH et EXO_KERNEL_A_MERKLE_ROOT via cargo build.
HashNotProvisioned,
```

---

## Vérification

```rust
// Test unitaire (feature = "exo-dev") :
#[test]
fn test_hashes_not_zero_in_release() {
    // En mode dev, les hashes peuvent être nuls.
    // Ce test vérifie que la const fn compile sans panique.
    let _ = A_IMAGE_HASH;
    let _ = A_MERKLE_ROOT;
}
```

---

**Priorité :** À corriger avant tout test ExoPhoenix sur bare-metal.  
**Référence :** `forge.rs:38,42`, `build.rs:10–30`
