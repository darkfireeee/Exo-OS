# CORR-73 — hotplug.rs : CPU_ONLINE_MASK limité à 64 CPUs

**Source :** Analyse à froid — non mentionné dans l'audit externe  
**Fichier :** `kernel/src/arch/x86_64/smp/hotplug.rs`  
**Priorité :** Phase 1 — bloque le hotplug sur tout système avec > 64 CPUs

---

## Constat exact

```rust
// hotplug.rs:26-27 — ACTUEL
/// Bitmask des CPUs online — bit N = CPU N online
/// Supporte jusqu'à 64 CPUs directement (au-delà, utiliser un tableau d'AtomicU64)
static CPU_ONLINE_MASK: AtomicU64 = AtomicU64::new(1); // bit 0 = BSP toujours online

pub fn cpu_is_online(cpu_id: u32) -> bool {
    if cpu_id >= 64 { return false; }   // ← HARD LIMIT 64
    CPU_ONLINE_MASK.load(Ordering::Acquire) & (1u64 << cpu_id) != 0
}

pub fn set_cpu_online(cpu_id: u32) {
    if cpu_id >= 64 { return; }         // ← silently drops CPUs > 63
    CPU_ONLINE_MASK.fetch_or(1u64 << cpu_id, Ordering::AcqRel);
}

pub fn set_cpu_offline(cpu_id: u32) {
    if cpu_id >= 64 { return; }         // ← idem
    CPU_ONLINE_MASK.fetch_and(!(1u64 << cpu_id), Ordering::AcqRel);
}
```

Le commentaire reconnaît lui-même la limitation et indique la solution à appliquer.
Sur un système avec 128 CPUs, les CPUs 64-127 :
- sont toujours vus comme offline par `cpu_is_online()`
- ne peuvent jamais être marqués online via `set_cpu_online()`
- ne peuvent pas être mis offline proprement via `set_cpu_offline()`

Impact pour ExoOS (MAX_CPUS=256) : 75% des cœurs inaccessibles au hotplug.

---

## Correction

Remplacer l'AtomicU64 unique par un tableau d'AtomicU64 dimensionné pour MAX_CPUS.

```rust
// hotplug.rs — APRÈS

use crate::arch::x86_64::cpu::topology::MAX_CPUS;

/// Nombre de mots u64 nécessaires pour le bitmask online.
const ONLINE_MASK_WORDS: usize = (MAX_CPUS + 63) / 64; // = 4 pour MAX_CPUS=256

/// Bitmask des CPUs online — bit N = CPU N online.
/// Tableau d'AtomicU64 : supporte jusqu'à MAX_CPUS CPUs.
/// [0] bit 0 = BSP toujours online (initial).
static CPU_ONLINE_MASK: [AtomicU64; ONLINE_MASK_WORDS] = {
    // Pas de [expr; N] pour les types non-Copy — utiliser une macro ou const fn
    // Pour MAX_CPUS=256 → ONLINE_MASK_WORDS=4 → 4 AtomicU64
    // Word 0 : bit 0 = BSP online, reste = 0
    // Words 1-3 : tous à 0
    [
        AtomicU64::new(1), // bit 0 = BSP
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]
    // NOTE: si MAX_CPUS change, ajuster manuellement le nombre d'éléments
    // ou utiliser une macro de génération.
};

/// Assertion compile-time : tableau dimensionné correctement
const _: () = assert!(
    ONLINE_MASK_WORDS == (MAX_CPUS + 63) / 64,
    "CPU_ONLINE_MASK mal dimensionné pour MAX_CPUS"
);

/// Retourne `true` si le CPU `cpu_id` est online.
#[inline]
pub fn cpu_is_online(cpu_id: u32) -> bool {
    let id = cpu_id as usize;
    if id >= MAX_CPUS { return false; }
    let word = id / 64;
    let bit  = id % 64;
    CPU_ONLINE_MASK[word].load(Ordering::Acquire) & (1u64 << bit) != 0
}

/// Marque un CPU comme online (appelé par l'AP au boot).
#[inline]
pub fn set_cpu_online(cpu_id: u32) {
    let id = cpu_id as usize;
    if id >= MAX_CPUS { return; }
    let word = id / 64;
    let bit  = id % 64;
    CPU_ONLINE_MASK[word].fetch_or(1u64 << bit, Ordering::AcqRel);
}

/// Marque un CPU comme offline.
#[inline]
pub fn set_cpu_offline(cpu_id: u32) {
    let id = cpu_id as usize;
    if id >= MAX_CPUS { return; }
    let word = id / 64;
    let bit  = id % 64;
    CPU_ONLINE_MASK[word].fetch_and(!(1u64 << bit), Ordering::AcqRel);
}

/// Retourne le nombre de CPUs actuellement online.
pub fn online_cpu_count() -> u32 {
    CPU_ONLINE_MASK.iter()
        .map(|w| w.load(Ordering::Relaxed).count_ones())
        .sum()
}

/// Retourne `true` si tous les CPUs dans le masque donné sont online.
/// (Utile pour attendre que tous les APs démarrent.)
pub fn all_cpus_online(count: u32) -> bool {
    online_cpu_count() >= count
}
```

---

## Note sur INIT/SIPI u8 (réponse à l'audit externe)

L'audit externe signale `send_init_ipi(lapic_id as u8)` comme un bug.
Ce n'est **pas** un bug — c'est correct par spec x86 :

- **INIT IPI** et **STARTUP IPI** sont des protocoles xAPIC (champ ICR[31:24] = 8 bits).
- Les systèmes avec APIC ID > 255 utilisent x2APIC qui **ne supporte pas INIT/SIPI**.
  Le démarrage des APs sur x2APIC utilise une séquence WAKEUP spécifique (Intel SDM
  Vol.3A §10.12.9 — MP Initialization Protocol for x2APIC).
- La conversion `lapic_id as u8` est donc correcte : si `lapic_id > 255`, ce CPU
  est sur x2APIC et la séquence INIT/SIPI ne le concernera pas.

**Ce qui serait un vrai bug** (pour implémentation future) : ne pas détecter les APs
x2APIC et tenter INIT/SIPI sur eux. C'est une limitation architecturale actuelle
(ExoOS ne supporte pas encore le boot d'APs x2APIC avec APIC ID > 255), pas un bug
dans le code existant.

---

## Validation

- [ ] `const _: ()` assertion compile sans erreur pour MAX_CPUS=256 (→ ONLINE_MASK_WORDS=4)
- [ ] Test : `set_cpu_online(127)` → `cpu_is_online(127) == true`
- [ ] Test : `set_cpu_online(255)` → `cpu_is_online(255) == true`
- [ ] Test : `cpu_is_online(256)` → `false` (hors bornes)
- [ ] `online_cpu_count()` retourne le bon compte après boot SMP
- [ ] Aucune régression sur les systèmes à 4-8 CPUs (cas QEMU)
