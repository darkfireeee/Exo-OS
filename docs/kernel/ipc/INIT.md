# Initialisation IPC — `ipc_init()` et séquence de boot

## Signature

```rust
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32)
```

## Paramètres

| Paramètre | Type | Description |
|---|---|---|
| `shm_base_phys` | `u64` | Adresse physique de base pour le pool SHM pré-alloué |
| `n_numa_nodes` | `u32` | Nombre de nœuds NUMA dans le système (≥ 1) |

## Position dans la séquence de boot

`ipc_init()` doit être appelée après les étapes obligatoires suivantes :

```
boot
 │
 ├─ 1. Initialisation de la mémoire physique (memory/phys)
 │       memory_init() — allocateur de pages physiques
 │
 ├─ 2. Initialisation du VMM (memory/vmm)
 │       vmm_init() — tables de pages, physmap
 │
 ├─ 3. Initialisation de la table futex (memory/utils/futex_table)
 │       futex_table_init() — FUTEX_TABLE global
 │
 ├─ 4. Initialisation du scheduler (scheduler/)
 │       scheduler_init() — ProcessId, contextes thread
 │
 ├─ 5. Initialisation de la sécurité (security/capability)
 │       capability_init() — CapTable, système de droits
 │
 └─ 6. Initialisation IPC ← ICI
         ipc_init(shm_base_phys, n_numa_nodes)
```

## Ce que fait `ipc_init()`

### Étape 1 — Pool SHM

```rust
shared_memory::pool::init_shm_pool(shm_base_phys);
```

Pré-alloue `IPC_MAX_CHANNELS * RING_SIZE` pages SHM à l'adresse physique fournie. Toutes les pages sont initialisées avec `PageFlags::SHM_DEFAULT` (`NO_COW | PINNED | READ | WRITE | SHARED`).

Les allocations ultérieures à `pool_alloc()` sont garanties < 100 ns car elles ne font que dépiler une entrée du pool.

### Étape 2 — Affinité NUMA

```rust
shared_memory::numa_aware::numa_init(n_numa_nodes as usize);
```

Initialise la structure de localité NUMA pour les allocations SHM. Chaque nœud NUMA dispose de sa propre portion du pool, limitant les accès mémoire distants lors des communications entre processus sur la même socket CPU.

Si `n_numa_nodes == 1`, la structure NUMA est un no-op (toutes allocations depuis nœud 0).

### Étape 3 — Compteurs de statistiques

Remet à zéro `IPC_STATS` (les compteurs `AtomicU64` sont déjà à 0 après BSS init, mais un reset explicite garantit la cohérence après un warm restart).

## Contraintes d'appel unique

`ipc_init()` **ne doit être appelée qu'une seule fois**. Un double appel :
- Réinitialise le pool SHM, libérant des pages encore mappées → corruption mémoire
- Remet à zéro les compteurs NUMA mid-flight → assertions échouées

## Paramètre `shm_base_phys`

L'adresse physique de base doit :
1. Être alignée sur 4 096 octets (4 KiB — alignement page)
2. Pointer vers une région de mémoire **réservée pour SHM IPC** dans la memory map
3. Ne pas chevaucher le pool de pages kernel, le stack, les buffers DMA, ou le code kernel

La taille requise est :
```
taille = IPC_MAX_CHANNELS × RING_SIZE × 4096 octets
       = 4096 × 16 × 4096
       = 268 435 456 octets (256 MiB)
```

## Ordre de dépendance strict

```
ipc_init() appelle :
  ├── init_shm_pool()
  │     └── utilise PhysAddr (memory/phys doit être init)
  └── numa_init()
        └── lit la topologie NUMA (scheduler/topology doit être init)
```

Si l'une de ces dépendances n'est pas initialisée, le comportement est indéfini (panique en debug, corruption silencieuse en release).

## Exemple d'appel dans `kernel/src/main.rs`

```rust
fn kernel_main(boot_info: &BootInfo) -> ! {
    // Couche 0
    memory::init(boot_info);
    memory::utils::futex_table::futex_table_init();

    // Couche 1
    scheduler::init();

    // Sécurité
    security::capability::init();

    // Couche 2a — IPC
    let shm_base = boot_info.shm_region.start_phys;
    let n_numa   = boot_info.numa_node_count;
    ipc::ipc_init(shm_base, n_numa);

    // Suite du boot...
    loop { scheduler::run() }
}
```
