# CORR-58 — ssr.rs : Adresse physique SSR_BASE utilisée comme pointeur virtuel (CRIT-06)

**Sévérité :** 🔴 CRITIQUE — BLOQUANT IMMÉDIAT (page fault au premier accès SSR)  
**Fichiers :** `kernel/src/exophoenix/ssr.rs`, `libs/exo-phoenix-ssr/src/lib.rs`, `kernel/src/exophoenix/sentinel.rs`  
**Impact :** UB garanti / page fault dès le premier appel à `ssr_atomic()` ou `ssr_atomic_u32()`

---

## Problème

```rust
// libs/exo-phoenix-ssr/src/lib.rs:27
pub const SSR_BASE_PHYS: u64 = 0x0100_0000; // adresse PHYSIQUE

// kernel/src/exophoenix/ssr.rs:5–6
pub use exo_phoenix_ssr::{
    SSR_BASE_PHYS as SSR_BASE,  // ← réexportée comme "SSR_BASE"
};

// kernel/src/exophoenix/ssr.rs:39 — UTILISÉE COMME POINTEUR VIRTUEL
unsafe fn ssr_atomic(offset: usize) -> &'static AtomicU64 {
    &*((SSR_BASE as usize + offset) as *const AtomicU64)
    //   ^^^^^^^^^^^^ 0x0100_0000 = adresse physique brute
    //   Sur un kernel avec physmap à 0xFFFF_8000_0000_0000 → page fault garanti
}
```

Le même bug apparaît dans `sentinel.rs` :
```rust
// sentinel.rs (extrait)
const SSR_CMD_PHYS_START: u64 = ssr::SSR_BASE + ssr::SSR_CMD_B2A as u64;
// ssr::SSR_BASE = 0x0100_0000 (physique) → accès en adresse physique brute
```

---

## Correction

### Étape 1 — Séparer proprement physique et virtuel dans la lib SSR

```rust
// libs/exo-phoenix-ssr/src/lib.rs

/// Adresse PHYSIQUE de la SSR.
/// Utilisée pour l'IOMMU, le mapping initial, et les accès depuis Kernel B
/// avant que la physmap soit établie (early boot uniquement).
pub const SSR_BASE_PHYS: u64 = 0x0100_0000;

/// Taille totale de la région SSR en bytes.
pub const SSR_SIZE_BYTES: usize = 0x10000; // 64 KB

// NOTE: SSR_BASE_VIRT n'est PAS défini ici car il dépend de PHYS_MAP_BASE
// qui est une constante du kernel, pas de la lib partagée.
// L'adresse virtuelle doit être calculée dans le kernel via :
//   PHYS_MAP_BASE + SSR_BASE_PHYS
```

### Étape 2 — Corriger `ssr.rs` dans le kernel

```rust
// kernel/src/exophoenix/ssr.rs

use crate::memory::core::PHYS_MAP_BASE;
use exo_phoenix_ssr::SSR_BASE_PHYS;

/// Adresse VIRTUELLE de la SSR dans la physmap du kernel.
/// Calculée une seule fois — PHYS_MAP_BASE est une constante de layout.
///
/// # SAFETY
/// Valide uniquement après que la physmap est établie (étape 11 du boot).
/// Ne jamais utiliser cette constante dans les étapes boot 1–10.
#[inline(always)]
fn ssr_base_virt() -> usize {
    // PHYS_MAP_BASE est une VirtAddr — son as_u64() est l'adresse virtuelle
    // de base de la physmap (ex: 0xFFFF_8000_0000_0000).
    (PHYS_MAP_BASE.as_u64() + SSR_BASE_PHYS) as usize
}

/// Accède à un AtomicU64 dans la SSR à l'offset donné.
///
/// # SAFETY
/// - `offset` doit être aligné sur 8 bytes
/// - `offset + 8 <= SSR_SIZE_BYTES`
/// - La physmap doit être initialisée
#[inline(always)]
pub unsafe fn ssr_atomic(offset: usize) -> &'static core::sync::atomic::AtomicU64 {
    debug_assert!(offset % 8 == 0, "SSR offset must be 8-byte aligned");
    debug_assert!(offset + 8 <= exo_phoenix_ssr::SSR_SIZE_BYTES,
                  "SSR offset out of bounds");
    &*((ssr_base_virt() + offset) as *const core::sync::atomic::AtomicU64)
}

/// Accède à un AtomicU32 dans la SSR à l'offset donné.
///
/// # SAFETY
/// - `offset` doit être aligné sur 4 bytes
/// - `offset + 4 <= SSR_SIZE_BYTES`
#[inline(always)]
pub unsafe fn ssr_atomic_u32(offset: usize) -> &'static core::sync::atomic::AtomicU32 {
    debug_assert!(offset % 4 == 0, "SSR offset must be 4-byte aligned");
    debug_assert!(offset + 4 <= exo_phoenix_ssr::SSR_SIZE_BYTES,
                  "SSR offset out of bounds");
    &*((ssr_base_virt() + offset) as *const core::sync::atomic::AtomicU32)
}

// SUPPRIMER la ligne suivante qui cause la confusion :
// pub use exo_phoenix_ssr::{ SSR_BASE_PHYS as SSR_BASE, ... };
//
// REMPLACER PAR :
pub use exo_phoenix_ssr::SSR_BASE_PHYS; // nommé explicitement PHYS
```

### Étape 3 — Corriger `sentinel.rs`

```rust
// kernel/src/exophoenix/sentinel.rs

// AVANT (bug) :
// const SSR_CMD_PHYS_START: u64 = ssr::SSR_BASE + ssr::SSR_CMD_B2A as u64;

// APRÈS — utiliser ssr_atomic() directement, sans construire d'adresse :
// Les accès SSR passent tous par les fonctions ssr_atomic()/ssr_atomic_u32().
// Ne pas recalculer les adresses manuellement depuis sentinel.rs.

// Exemple de lecture correcte du canal B2A :
fn read_ssr_cmd_b2a() -> u32 {
    unsafe {
        crate::exophoenix::ssr::ssr_atomic_u32(
            exo_phoenix_ssr::SSR_CMD_B2A as usize
        ).load(core::sync::atomic::Ordering::Acquire)
    }
}
```

### Étape 4 — Accès early boot (avant physmap)

```rust
// Pour les étapes boot 1–10 où la physmap n'est pas encore établie,
// et où Kernel B doit accéder à la SSR via identity map ou accès direct :

/// Accède à la SSR en mode early boot (identity mapping ou accès direct physique).
/// À utiliser UNIQUEMENT dans les étapes de boot 1–10.
///
/// # SAFETY
/// Valide uniquement si la SSR est mappée en identity (physique = virtuel)
/// pendant early boot. NE PAS utiliser après init de la physmap.
#[cfg(feature = "early-ssr-access")]
pub unsafe fn ssr_atomic_early(offset: usize) -> &'static core::sync::atomic::AtomicU64 {
    &*((SSR_BASE_PHYS as usize + offset) as *const core::sync::atomic::AtomicU64)
}
```

---

## Impact sur le reste du codebase

Rechercher et corriger toutes les occurrences de `SSR_BASE` (ancien alias) :

```bash
grep -rn "ssr::SSR_BASE\b" kernel/src/
# Remplacer par : accès via ssr_atomic(offset)
# Ne jamais construire d'adresse manuelle depuis SSR_BASE_PHYS
```

---

## Test de non-régression

```rust
#[test]
#[cfg(test)]
fn test_ssr_virt_addr_not_phys() {
    // En test (hosted), PHYS_MAP_BASE != 0
    // Vérifier que ssr_base_virt() != SSR_BASE_PHYS
    let virt = unsafe { super::ssr_base_virt() };
    assert_ne!(virt as u64, exo_phoenix_ssr::SSR_BASE_PHYS,
        "SSR_BASE_VIRT should not equal SSR_BASE_PHYS");
}
```

---

**Priorité :** PREMIÈRE CORRECTION à appliquer — tous les autres bugs SSR sont masqués derrière celui-ci.  
**Référence :** `ssr.rs:6,39,47`, `sentinel.rs` (toutes les constantes SSR_*)
