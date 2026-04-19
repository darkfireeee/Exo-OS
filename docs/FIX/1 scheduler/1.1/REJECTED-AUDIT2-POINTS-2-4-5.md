# REJETS AUDIT POST-FIX1 — Points 2, 4, 5

---

## REJETÉ — Point 2 : CpuSet [u64;8] non mis à jour

**Claim :** "Vous avez étendu le stockage à [u64;8] dans le TCB, mais les fonctions
manipulant cette affinité n'ont pas été adaptées."

**Verdict : ENTIÈREMENT FAUX**

### Code réel vérifié

```rust
// affinity.rs:15 — ACTUEL
const MASK_WORDS: usize = 4;  // 4 × 64 = 256 bits
pub struct CpuSet { pub(crate) bits: [u64; MASK_WORDS] }
```

Le CpuSet est `[u64; 4]` = 256 bits = **256 CPUs = MAX_CPUS**. C'est correct.
Il n'existe nulle part de `[u64; 8]` dans ce codebase pour l'affinité.

### Toutes les fonctions sont correctes

`cpu_affinity_mask()` dans task.rs :
```rust
CpuSet::new([
    self.cpu_affinity.load(Ordering::Acquire),           // bits 0-63
    self.affinity_ext_word(1).load(Ordering::Acquire),   // bits 64-127
    self.affinity_ext_word(2).load(Ordering::Acquire),   // bits 128-191
    self.affinity_ext_word(3).load(Ordering::Acquire),   // bits 192-255
])
```

`set_cpu_affinity_mask()` écrit les 4 mots en Release. Correct.

`CpuSet::contains()` : `self.bits[cpu / 64] & (1u64 << (cpu % 64))` — correct.

`CpuSet::set()` / `clear()` : indexation `[cpu / 64]` — correct.

`CpuSet::first_cpu()` : itère sur tous les 4 mots — correct.

**L'audit externe a confondu [u64;4] avec [u64;8], ou s'est basé sur un état
antérieur du code qui n'existe pas dans le dépôt.**

---

## REJETÉ — Point 4 : Initialisation des runqueues hardcodée

**Claim :** "Vérifier si la boucle d'initialisation des runqueues boucle jusqu'à
MAX_CPUS (512) ou utilise une vieille limite."

**Verdict : FAUX**

```rust
// runqueue.rs:667 — ACTUEL
for i in 0..nr_cpus.min(MAX_CPUS) {
```

`nr_cpus` est le nombre de CPUs détectés au boot (via MADT/ACPI).
`MIN(nr_cpus, MAX_CPUS)` garantit qu'on ne dépasse jamais les bornes statiques.
Il n'y a aucune limite hardcodée.

---

## REJETÉ (partiellement) — Point 5 : INIT/SIPI u8 truncation

**Claim :** "La fonction send_ipi(cpu_id) gère-t-elle les IDs élevés sans overflow?"

**Verdict : NON-BUG pour INIT/SIPI — explication spec**

INIT IPI et STARTUP IPI sont des protocoles **xAPIC uniquement** (Intel SDM Vol.3A §10.6.1).
Le champ de destination dans l'ICR xAPIC est de 8 bits (bits 31:24 du registre ICR_HIGH).
La signature `send_init_ipi(dest_apic_id: u8)` est donc correcte par spec.

Les systèmes avec APIC ID > 255 utilisent x2APIC, pour lequel le protocole de boot
des APs est différent (WAKEUP mailbox, non implémenté dans ExoOS Phase 1). Ce n'est
pas un bug — c'est une limitation architecturale documentée de la Phase 1.

**Ce qui est un vrai bug** (non demandé par l'audit, découvert lors de l'analyse) :
`hotplug.rs::CPU_ONLINE_MASK: AtomicU64` limite le hotplug à 64 CPUs. Corrigé par CORR-73.

---

## Résumé des corrections valides issues de cet audit

| ID | Fichier | Type | Priorité |
|----|---------|------|----------|
| CORR-72 | numa_affinity.rs, heap/cache.rs | Constantes locales → import + assertion | Phase 2 |
| CORR-73 | hotplug.rs | CPU_ONLINE_MASK AtomicU64 → [AtomicU64; 4] | Phase 1 |
| CORR-74 | runqueue.rs, migration.rs | Relaxed → Acquire/Release sur vruntime + cpu_id | Phase 1 |
