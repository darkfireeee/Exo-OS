# CORR-57 — Affinité CPU : étendre de 64 bits à 256 bits

**Source :** Audit Qwen (P1-03)  
**Fichiers :** `kernel/src/scheduler/smp/affinity.rs`, `kernel/src/scheduler/core/task.rs`  
**Priorité :** Phase 1

---

## Constat

`affinity.rs` représente le masque d'affinité CPU comme un `u64`, limitant le scheduling
à 64 CPUs maximum. ExoOS cible MAX_CPUS=256 → 192 CPUs inutilisables.

```rust
// affinity.rs:58 — BIT OVERFLOW silencieux pour cpu >= 64
pub fn cpu_allowed(affinity: u64, cpu: CpuId) -> bool {
    if cpu.0 as usize >= 64 { return false; }  // limite implicite
    affinity & (1u64 << cpu.0) != 0
}
```

---

## Correction

### Nouveau type CpuSet (affinity.rs)

```rust
/// Masque d'affinité CPU pour MAX_CPUS = 256.
/// 4 × u64 = 256 bits. Opérations const-friendly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSet {
    bits: [u64; 4],
}

impl CpuSet {
    pub const EMPTY: Self = Self { bits: [0; 4] };
    pub const ALL: Self = Self { bits: [u64::MAX; 4] };

    #[inline]
    pub const fn new(bits: [u64; 4]) -> Self { Self { bits } }

    /// Retourne un CpuSet avec uniquement le CPU `id` activé.
    #[inline]
    pub fn single(cpu: CpuId) -> Self {
        let mut s = Self::EMPTY;
        s.set(cpu);
        s
    }

    #[inline]
    pub fn set(&mut self, cpu: CpuId) {
        let id = cpu.0 as usize;
        if id < MAX_CPUS {
            self.bits[id / 64] |= 1u64 << (id % 64);
        }
    }

    #[inline]
    pub fn clear(&mut self, cpu: CpuId) {
        let id = cpu.0 as usize;
        if id < MAX_CPUS {
            self.bits[id / 64] &= !(1u64 << (id % 64));
        }
    }

    #[inline]
    pub fn contains(&self, cpu: CpuId) -> bool {
        let id = cpu.0 as usize;
        if id >= MAX_CPUS { return false; }
        self.bits[id / 64] & (1u64 << (id % 64)) != 0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    /// Intersection (AND)
    #[inline]
    pub fn and(&self, other: &Self) -> Self {
        Self {
            bits: [
                self.bits[0] & other.bits[0],
                self.bits[1] & other.bits[1],
                self.bits[2] & other.bits[2],
                self.bits[3] & other.bits[3],
            ]
        }
    }

    /// Premier CPU disponible (lowest set bit), ou None si vide.
    pub fn first_cpu(&self) -> Option<CpuId> {
        for (i, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                return Some(CpuId((i * 64 + word.trailing_zeros() as usize) as u32));
            }
        }
        None
    }
}
```

### Remplacement dans task.rs (champ TCB)

```rust
// AVANT — dans ThreadControlBlock
pub affinity: u64,

// APRÈS
pub affinity: CpuSet,
```

**Note layout TCB :** `u64` → `CpuSet([u64; 4])` = 32 bytes. Le TCB GI-01 est 256B,
vérifier que ce changement ne déborde pas. Si le champ `affinity` actuel est dans
la zone froide (`_cold_reserve`), l'extension est gratuite. Sinon, ajuster le layout
et mettre à jour les assertions `size_of::<TCB>() == 256`.

### Fonctions utilitaires à mettre à jour

```rust
// affinity.rs — cpu_allowed remplacé par CpuSet::contains
// AVANT
pub fn cpu_allowed(affinity: u64, cpu: CpuId) -> bool { ... }

// APRÈS
// Utiliser directement : tcb.affinity.contains(cpu)
// Garder un alias de compatibilité le temps de la migration :
#[deprecated = "utiliser CpuSet::contains directement"]
pub fn cpu_allowed_compat(affinity: &CpuSet, cpu: CpuId) -> bool {
    affinity.contains(cpu)
}

// affinity_mask_from_cpu_mask — adapter pour CpuSet
pub fn affinity_mask_from_cpu_mask(mask: &CpuMask) -> CpuSet {
    let mut set = CpuSet::EMPTY;
    for cpu in mask.iter() {
        set.set(cpu);
    }
    set
}

// sanitize_affinity — adapter
pub fn sanitize_affinity(affinity: CpuSet) -> CpuSet {
    if affinity.is_empty() { CpuSet::ALL } else { affinity }
}
```

---

## Validation

- [ ] `static_assert!(core::mem::size_of::<CpuSet>() == 32)`
- [ ] `static_assert!(core::mem::size_of::<ThreadControlBlock>() == 256)` — toujours valide
- [ ] Test : thread sur CPU 65, 127, 255 → schedulé correctement
- [ ] Test : `CpuSet::ALL.contains(CpuId(255)) == true`
- [ ] Test : `CpuSet::EMPTY.first_cpu() == None`
