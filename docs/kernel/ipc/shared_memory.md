# Mémoire Partagée IPC

Le sous-module `ipc/shared_memory/` gère l'allocation, le mapping et le cycle de vie des pages de mémoire partagée entre processus.

## Vue d'ensemble

```
shared_memory/
├── page.rs        — Structure ShmPage, flags NO_COW + PINNED (IPC-03)
├── pool.rs        — Pool pré-alloué de pages (alloc < 100 ns)
├── descriptor.rs  — ShmDescriptor (ID, taille, permissions, adresses mappées)
├── mapping.rs     — shm_map/unmap, hooks de mapping
├── allocator.rs   — Allocateur SHM lock-free
└── numa_aware.rs  — Affinité NUMA pour allocations locales
```

---

## Pages SHM — `shared_memory/page.rs`

### Flags obligatoires (IPC-03)

```rust
pub struct PageFlags(u32);
impl PageFlags {
    pub const READ:      Self = Self(1 << 0);
    pub const WRITE:     Self = Self(1 << 1);
    pub const EXECUTE:   Self = Self(1 << 2);
    pub const NO_COW:    Self = Self(1 << 3);  // ← OBLIGATOIRE
    pub const PINNED:    Self = Self(1 << 4);  // ← OBLIGATOIRE
    pub const SHARED:    Self = Self(1 << 5);

    /// Flags par défaut pour toute page SHM IPC.
    pub const SHM_DEFAULT: Self = Self(
        Self::READ.0 | Self::WRITE.0 | Self::NO_COW.0
        | Self::PINNED.0 | Self::SHARED.0
    );
}
```

**`NO_COW`** : interdit la copie sur écriture (Copy-on-Write). Les deux processus voient exactement les mêmes données physiques. Sans ce flag, le VMM pourrait créer des copies privées lors d'une écriture, rompant la communication.

**`PINNED`** : interdit la migration ou l'éviction de la page. La page ne peut pas être swappée, ni déplacée par le mécanisme NUMA balancing, pendant qu'une communication IPC est active.

### Structure ShmPage

```rust
#[repr(C, align(64))]
pub struct ShmPage {
    phys_addr:       u64,           // adresse physique alignée 4 KiB
    refcount:        AtomicU32,     // nombre de mappings actifs
    flags:           PageFlags,
    pool_index:      u32,           // index dans le pool
    generation:      u32,           // génération (détection use-after-free)
    last_mapper_pid: u32,           // PID du dernier mappeur (debug)
    alloc_ts:        u64,           // timestamp d'allocation (NUMA eviction)
    reuse_count:     u32,           // compteur de réutilisations
    _pad:            [u8; ...],
}
```

### Cycle de vie d'une page

```
init()           — Initialise les champs, refcount=0, flags=SHM_DEFAULT
  │
  ▼
acquire()        — refcount.fetch_add(1) — mapping actif
  │
  ▼ (utilisation par le processus)
  │
release()        — refcount.fetch_sub(1)
                 — si refcount == 0 → page libre → retour au pool
```

### API

```rust
impl ShmPage {
    pub fn init(&mut self)
    pub unsafe fn set_phys_unchecked(&mut self, addr: u64)
    pub fn acquire(&self) -> u32      // retourne nouveau refcount
    pub fn release(&self) -> bool     // true si page libérée
    pub fn is_free(&self) -> bool
    pub fn phys(&self) -> PhysAddr
    pub fn flags(&self) -> PageFlags
}
```

---

## Pool pré-alloué — `shared_memory/pool.rs`

### Description

Pool statique de `ShmPage` pré-alloué au boot. Les allocations sont atomiques et garanties < 100 ns (pas d'allocateur générique, pas de verrou).

### Initialisation

```rust
pub fn init_shm_pool(base_phys: u64)
```

Appelée depuis `ipc_init()`. Initialise toutes les pages du pool à leur adresse physique respective :
```
page[i].phys_addr = base_phys + i * 4096
page[i].flags     = PageFlags::SHM_DEFAULT
page[i].refcount  = 0  (libre)
```

### Allocation / libération

```rust
pub fn pool_alloc() -> Option<&'static mut ShmPage>
pub fn pool_free(page: &mut ShmPage)
```

Algorithme : stack lock-free via `AtomicUsize` pointant vers le sommet. `pool_alloc()` tente de dépiler avec `compare_exchange`. `pool_free()` empile.

### Taille du pool

```
MAX_POOL_PAGES = IPC_MAX_CHANNELS × RING_SIZE = 4096 × 16 = 65 536 pages
```

---

## Descripteur SHM — `shared_memory/descriptor.rs`

### Description

Objet kernel représentant une région SHM allouée. Maintient la liste des adresses virtuelles mappées dans chaque espace d'adressage.

### Structure

```rust
pub struct ShmDescriptor {
    id:          u64,                     // identifiant unique
    size:        usize,                   // taille en octets
    perms:       PageFlags,               // permissions RWX
    phys_pages:  [*mut ShmPage; 16],      // pages physiques sous-jacentes
    n_pages:     usize,
    mappings:    [(ProcessId, *mut u8); 8], // mappings actifs par PID
    n_mappings:  usize,
}
```

---

## Mapping — `shared_memory/mapping.rs`

### Description

Mappe/démappe une région SHM dans l'espace d'adressage d'un processus. Supporte des hooks pour notifier d'autres sous-systèmes (ex. : driver GPU).

### API

```rust
pub fn shm_map(ptr: *mut u8, pid: ProcessId) -> Result<*mut u8, IpcError>
pub fn shm_unmap(ptr: *mut u8, pid: ProcessId) -> Result<(), IpcError>

// Hooks optionnels
pub fn register_map_hook(f: fn(*mut u8, ProcessId) -> Result<(), IpcError>)
pub fn register_unmap_hook(f: fn(*mut u8, ProcessId))
```

### Opération de mapping

1. Retrouve le `ShmDescriptor` associé à `ptr`
2. Alloue des entrées dans les tables de pages du processus `pid`
3. Positionne `NO_COW + PINNED` dans les PTEs
4. `ShmPage::acquire()` pour chaque page
5. Appelle le hook si enregistré
6. Retourne l'adresse virtuelle dans l'espace de `pid`

---

## Allocateur lock-free — `shared_memory/allocator.rs`

### Description

Interface de haut niveau pour allouer et libérer des régions SHM de taille quelconque (arrondie à la page).

### API

```rust
pub fn shm_alloc(size: usize) -> Result<*mut u8, IpcError>
pub fn shm_free(ptr: *mut u8) -> Result<(), IpcError>
```

### Algorithme

1. Calcule `n_pages = ceil(size / 4096)`
2. Appelle `pool_alloc()` × n_pages
3. Crée un `ShmDescriptor` avec les pages allouées
4. Retourne un pointeur vers la première page (physmap)

Si le pool est vide : retourne `Err(IpcError::ShmPoolFull)`.

---

## Affinité NUMA — `shared_memory/numa_aware.rs`

### Description

Optimise les allocations SHM pour minimiser les accès mémoire distants sur les systèmes multi-socket.

### Initialisation

```rust
pub fn numa_init(n_nodes: usize)
```

Divise le pool SHM en `n_nodes` tranches de taille égale, une par nœud NUMA.

### Allocation locale

```rust
pub fn numa_local_alloc(node_hint: usize) -> Option<&'static mut ShmPage>
```

Tente d'allouer depuis la tranche du nœud `node_hint`. Si la tranche est vide, replie sur le nœud 0 (fallback global).

### Politique de non-migration

Les pages SHM étant `PINNED`, le mécanisme de NUMA balancing du kernel ne peut pas les déplacer vers un autre nœud pour affinie. L'affinité est donc décidée **une seule fois** à l'allocation.

---

## Séquence de vie d'une région SHM

```
ipc_init(base_phys, n_numa)
  └── init_shm_pool(base_phys)     pool de 65 536 pages prêtes

Producteur:
  shm_alloc(size)
    └── pool_alloc() × n_pages
    └── ShmDescriptor créé
    └── Retourne ptr (physmap)

Producteur:
  shm_map(ptr, pid_consommateur)
    └── PTEs consommateur → pages physiques (NO_COW + PINNED)
    └── ShmPage::acquire() × n_pages

Producteur écrit, Consommateur lit (zero copie)

Consommateur:
  shm_unmap(ptr, pid_consommateur)
    └── Supprime PTEs consommateur
    └── ShmPage::release() × n_pages

Producteur:
  shm_free(ptr)
    └── ShmPage::release() × n_pages
    └── Si refcount == 0 → pool_free() × n_pages
```
