# Scheduler SMP — Load Balancing, Migration, Affinité, Topologie

> **Sources** : `kernel/src/scheduler/smp/`  
> **Règles** : SCHED-10, SCHED-01

---

## Table des matières

1. [topology.rs — Carte CPU/NUMA](#1-topologyrs--carte-cpunuma)
2. [affinity.rs — CpuMask](#2-affinityrs--cpumask)
3. [migration.rs — Migration IPI](#3-migrationrs--migration-ipi)
4. [load_balance.rs — Équilibrage de charge](#4-load_balancers--équilibrage-de-charge)

---

## 1. topology.rs — Carte CPU/NUMA

### Constantes

```rust
pub const MAX_CPUS:  usize = 256;  // CPUs logiques maximum
pub const MAX_NODES: usize = 16;   // Nœuds NUMA maximum
pub const CPU_ABSENT: u32 = u32::MAX;  // Valeur sentinelle CPU absent
```

### Initialisation

```rust
pub unsafe fn init(nr_cpus: usize, nr_nodes: usize)
```
- Initialise `NR_CPUS` et `NR_NODES` atomiquement.
- Met `CPU_NODE_MAP[0..nr_cpus]` à 0 (nœud 0 par défaut).
- Met `NUMA_DISTANCE[i][i] = 10` (distance intra-nœud standard ACPI).

### Configuration

```rust
// Associe un CPU à un nœud NUMA
pub unsafe fn set_cpu_node(cpu: CpuId, node: u8)

// Déclare deux CPUs comme HyperThreading siblings
pub unsafe fn set_cpu_sibling(cpu: CpuId, sibling: CpuId)

// Déclare la distance NUMA entre deux nœuds (de 10 = local à ~200 = remote)
pub unsafe fn set_numa_distance(a: usize, b: usize, dist: u8)
```

### Requêtes

```rust
pub fn nr_cpus()  -> usize
pub fn nr_nodes() -> usize
pub fn cpu_node(cpu: CpuId) -> u8         // Nœud NUMA du CPU
pub fn cpu_sibling(cpu: CpuId) -> u32     // Sibling HT (CPU_ABSENT si aucun)
pub fn numa_distance(a: usize, b: usize) -> u8
pub fn same_node(cpu: CpuId, reference: CpuId) -> bool
```

### Layout mémoire

```rust
static CPU_NODE_MAP:      [AtomicU8;  256]  // cpu → node
static CPU_SIBLING_MAP:   [AtomicU32; 256]  // cpu → sibling cpu
static NUMA_DISTANCE_TBL: [[AtomicU8; 16]; 16]  // node×node → distance
static NR_CPUS:  AtomicU32
static NR_NODES: AtomicU32
```

---

## 2. affinity.rs — CpuMask

### CpuMask

```rust
pub struct CpuMask {
    bits: [u64; 4],  // 256 bits → 256 CPUs max
}

impl CpuMask {
    pub const fn empty() -> Self           // Tous les bits à 0
    pub fn full() -> Self                  // Tous les CPUs en ligne
    pub fn set(&mut self, cpu: CpuId)     // bits[cpu/64] |= 1<<(cpu%64)
    pub fn clear(&mut self, cpu: CpuId)   // bits[cpu/64] &= !(1<<(cpu%64))
    pub fn test(&self, cpu: CpuId) -> bool
    pub fn first(&self) -> Option<CpuId>  // Premier CPU dans le masque
    pub fn count(&self) -> usize          // popcount total
    pub fn and(&self, other: &Self) -> Self
    pub fn is_empty(&self) -> bool
}
```

### Fonctions utilitaires

```rust
// Vérifie si le TCB est autorisé sur ce CPU
// (utilise tcb.affinity : u64, representant les CPUs 0-63)
pub fn cpu_allowed(affinity: u64, cpu: CpuId) -> bool {
    if cpu.0 >= 64 { return false; }
    affinity & (1u64 << cpu.0) != 0
}

// Convertit un CpuMask (256 bits) en affinity u64 (64 premiers CPUs)
pub fn affinity_mask_from_cpu_mask(mask: &CpuMask) -> u64

// Valide l'affinité : retire les CPUs hors-ligne ou invalides
pub fn sanitize_affinity(affinity: u64) -> u64
```

### Compteur

```rust
pub static AFFINITY_VIOLATIONS: AtomicU64  // Tentatives sur CPU non autorisé
```

---

## 3. migration.rs — Migration IPI

### FFI vers arch/

```rust
extern "C" {
    // Envoie un IPI (Inter-Processor Interrupt) de reschedule au CPU cible
    fn arch_send_reschedule_ipi(target_cpu: u32);

    // Retourne le numéro du CPU courant (via CPUID ou MSR_TSC_AUX)
    fn arch_current_cpu() -> u32;
}
```

### Compteurs

```rust
pub static MIGRATIONS_SENT:     AtomicU64
pub static MIGRATIONS_RECEIVED: AtomicU64
pub static MIGRATIONS_DROPPED:  AtomicU64
```

### request_migration

```rust
pub unsafe fn request_migration(
    tcb: NonNull<ThreadControlBlock>,
    target: CpuId,
)
```

1. Valide `target` :
   - `target.0 < nr_cpus()` (CPU valide)
   - `target != arch_current_cpu()` (pas une migration vers soi-même)
   - `cpu_allowed(tcb.affinity, target)` (affinité respectée)
2. Si invalide → `MIGRATIONS_DROPPED++`, return.
3. Retire le TCB de la run queue locale.
4. Insère dans `PENDING_MIGRATIONS[target]` (file lock-free).
5. `arch_send_reschedule_ipi(target.0)` → réveille le CPU cible.
6. `MIGRATIONS_SENT++`.

### drain_pending_migrations

```rust
pub unsafe fn drain_pending_migrations(
    cpu: CpuId,
    rq: &mut PerCpuRunQueue,
)
```

Appelé au début de `scheduler_tick()` sur le CPU cible :
1. Vide `PENDING_MIGRATIONS[cpu.0]`.
2. Pour chaque TCB reçu → `rq.enqueue(tcb)`.
3. `MIGRATIONS_RECEIVED += count`.

---

## 4. load_balance.rs — Équilibrage de charge

### Constantes

```rust
pub const BALANCE_INTERVAL_TICKS:     u64   = 4;   // Toutes les 4 ms (4 ticks @HZ=1000)
pub const IMBALANCE_THRESHOLD:        usize = 2;   // Différence min pour migrer
pub const MAX_MIGRATIONS_PER_BALANCE: usize = 4;   // Max migrations par cycle
```

### Compteurs

```rust
pub static BALANCE_RUNS:       AtomicU64  // Appels à balance_cpu()
pub static BALANCE_MIGRATIONS: AtomicU64  // Threads migrés
pub static BALANCE_NUMA_SKIP:  AtomicU64  // Migrations évitées (NUMA cross-node)
```

### balance_cpu (SCHED-10)

```rust
pub unsafe fn balance_cpu(local_cpu: CpuId)
```

**Algorithme** :

```
1. Pour chaque CPU remote != local_cpu :
   a. Calcule diff = remote.nr_running - local.nr_running
   b. Si diff <= IMBALANCE_THRESHOLD → skip
   c. Si !same_node(local, remote) && numa_distance > 40 → BALANCE_NUMA_SKIP++, skip

2. Pour min(diff/2, MAX_MIGRATIONS_PER_BALANCE) threads :
   a. lock remote_rq (IrqGuard)   ← SCHED-10 : lock src avant lock dst
   b. lock local_rq  (IrqGuard)
   c. tcb = remote_rq.cfs_dequeue_for_migration(local_cpu)
   d. Si tcb.affinity & local_cpu → local_rq.enqueue(tcb)
   e. Sinon → MIGRATIONS_DROPPED++
   f. Unlock IrqGuards (RAII)

3. BALANCE_RUNS++
4. BALANCE_MIGRATIONS += migrés
```

### Respect de SCHED-10 (Lock Ordering)

La règle SCHED-10 impose : **locks scheduler < locks memory**.

Dans `balance_cpu` :
- On acquiert `remote_rq.lock` **avant** `local_rq.lock` (ordre constant = toujours src avant dst).
- Jamais d'allocation mémoire (émergency_pool ou alloc) **sous** un lock de run queue.
- La migration n'appelle pas `emergency_pool_alloc_*` (les WaitNodes sont pré-alloués).

### NUMA-aware balancing

```
même nœud  (distance ≤ 20) : migration agressive (seuil = 2)
nœud voisin (distance ≤ 40) : migration modérée (seuil = 4)
nœud distant (distance > 40) : migration évitée (BALANCE_NUMA_SKIP++)
```
