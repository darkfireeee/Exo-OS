# Module NUMA — Documentation Technique
> Extrait de `kernel/src/memory/physical/numa/` et `kernel/src/memory/numa/`

---

## 1. Vue d'ensemble

Le sous-système NUMA (**Non-Uniform Memory Access**) est localisé dans
`kernel/src/memory/physical/numa/` avec un façade de compat dans
`kernel/src/memory/numa/`.

**Règle fondamentale : COUCHE 0** — aucune dépendance vers
`scheduler/`, `process/`, `ipc/`, `fs/`. Toute communication inverse
se fait par **injection de trait** (ex. `MigrationPageTableOps`).

---

## 2. Arborescence

```
memory/physical/numa/
├── mod.rs          Re-exports publics, fn init()
├── node.rs         NumaNode, NumaNodeTable, NUMA_NODES statique
├── distance.rs     NumaDistanceTable, NUMA_DISTANCE statique (ACPI SLIT)
├── policy.rs       NumaPolicy, NumaNodeMask, select_node()
└── migration.rs    migrate_page(), migrate_pages_batch(), MigrationPageTableOps

memory/numa/        (façade compat)
└── mod.rs          Re-exports + délègue à physical/numa/, fn init()
```

---

## 3. Structures principales

### 3.1 `NumaNode` (`node.rs`)
```
NumaNode (repr C, align 64)
  ├── id:          u32                    — identifiant (0..MAX_NUMA_NODES-1)
  ├── cpu_mask:    AtomicU64              — masque des CPUs locaux
  ├── range_count: u32                    — nombre de plages physiques
  ├── active:      AtomicBool             — nœud reconnu à l'init ACPI SRAT
  ├── stats:       NumaNodeStats          — compteurs live (local/remote/alloc/free)
  └── ranges:      [NumaPhysRange; 16]   — plages physiques du nœud
```

**`NumaNodeStats`** (align 64, tout atomique — Relaxed) :
| Champ          | Signification |
|----------------|---------------|
| `total_pages`  | Pages totales dans le nœud |
| `free_pages`   | Pages libres actuelles |
| `used_pages`   | Pages allouées |
| `local_allocs` | Allocations satisfaites localement |
| `remote_allocs`| Allocations sur nœud distant (fallback) |
| `migrated_in`  | Pages reçues par migration |
| `migrated_out` | Pages envoyées par migration |

### 3.2 `NumaNodeTable` (`node.rs`)
Table centrale statique :
```rust
pub static NUMA_NODES: NumaNodeTable = NumaNodeTable::new();
```
Méthodes :
- `register_node(cpu_mask) -> u32` — enregistre un nouveau nœud, retourne son id.
- `add_range(id, start, end) -> bool` — ajoute une plage physique au nœud `id`.
- `node_for_phys(phys) -> u32` — nœud propriétaire d'une adresse physique.
- `node_for_cpu(cpu_id) -> u32` — nœud local d'un CPU.
- `get(id) -> Option<&NumaNode>` — accès en lecture.
- `count() -> u32` — nombre de nœuds actifs.

### 3.3 `NumaDistanceTable` (`distance.rs`)
Matrice symétrique `MAX_NUMA_NODES × MAX_NUMA_NODES` peuplée depuis
le tableau ACPI **SLIT**. Valeurs par défaut :
```
NUMA_DISTANCE_LOCAL       = 10  (même nœud)
NUMA_DISTANCE_REMOTE      = 20  (1 hop)
NUMA_DISTANCE_FAR         = 40  (2 hops, topologie ring)
NUMA_DISTANCE_UNREACHABLE = 255 (sentinelle)
```

**Conformité IA-KERNEL-01** : la table est un `static` const — aucune
génération runtime de politique.

### 3.4 `NumaPolicy` (`policy.rs`)
```rust
pub enum NumaPolicy {
    Default,                   // nœud local du CPU courant
    Bind(NumaNodeMask),        // exclusif au masque de nœuds
    Preferred(u32),            // préféré nid, fallback croissant
    Interleave(NumaNodeMask),  // round-robin entre les nœuds du masque
}
```

`NumaNodeMask(u64)` : bitmask — bit `i` = nœud `i`, `MAX_NUMA_NODES = 8`.

### 3.5 Migration (`migration.rs`)
Interface de trait pour découpler memory/ de virtual/ :
```rust
pub trait MigrationPageTableOps {
    fn get_pte(&self, virt_addr: u64) -> Option<u64>;
    fn swap_pte(&self, virt_addr: u64, new_pte: u64) -> Result<u64, ()>;
    fn flush_tlb(&self, virt_addr: u64);
}
```

Algorithme de `migrate_page()` :
1. `alloc_pages()` sur le nœud cible.
2. `copy_frame()` via physmap (memcpy 4 KiB).
3. `swap_pte()` atomique + `flush_tlb()`.
4. `let _ = free_pages(src_frame, 0)` — libère la frame source.

---

## 4. Problème NUMA résolu — le pattern `addr_of!`

### 4.1 Le problème

`NumaNodeTable` expose `register_node(&self)` et `add_range(&self, …)`
avec un récepteur **`&self`** (référence partagée), alors que ces
méthodes doivent **muter** un slot précis du tableau `nodes`.

En contexte `static`, cela a d'abord été écrit :
```rust
// ❌ AVANT — UB : cast &T → *mut T via référence intermédiaire
let node = unsafe {
    &mut *( &self.nodes[id as usize] as *const NumaNode as *mut NumaNode )
};
```
Ce code viole la règle **`invalid_reference_casting`** de Rust : créer
un `*mut T` à partir d'un `&T` est un **comportement indéfini**,
car le compilateur suppose qu'une `&T` n'est jamais mutée.

### 4.2 La solution : `core::ptr::addr_of!`

```rust
// ✅ APRÈS — correct : addr_of! obtient le pointeur brut SANS &T intermédiaire
// SAFETY : accès par id unique (counter atomique), pas de race.
let node = unsafe {
    &mut *( core::ptr::addr_of!(self.nodes[id as usize]) as *mut NumaNode )
};
node.cpu_mask.store(cpu_mask, Ordering::Release);
node.active.store(true, Ordering::Release);
```

`addr_of!(expr)` produit un `*const T` **sans créer de référence** vers
l'emplacement mémoire. Le cast vers `*mut T` n'entre donc pas en
conflit avec les garanties d'aliasing de `&T`.

### 4.3 Garantie d'exclusivité (pas de race)

La sécurité repose sur **trois invariants** :

| Invariant | Mécanisme |
|-----------|-----------|
| Chaque slot écrit exactement une fois | `node_count.fetch_add(1, AcqRel)` alloue un id unique |
| Visibilité des écritures sur le slot | `Ordering::Release` sur `cpu_mask` et `active` |
| Lecture cohérente par les autres CPUs | `Ordering::Acquire` sur `node_count.load()` dans `node_for_cpu` |

### 4.4 Champs atomiques vs pointeur brut

Les champs qui varient **après** l'init (`free_pages`, `used_pages`,
`local_allocs`…) sont des `AtomicU64` — pas besoin de pointeur brut.
Seul l'accès initial d'enregistrement (`register_node`, `add_range`)
requiert le pattern `addr_of!` pour initialiser le slot.

---

## 5. Ordre d'initialisation

```
Phase 8 : memory/physical/numa/init()
  └── NUMA_NODES.register_node(u64::MAX)    // nœud 0 par défaut (UMA)
  └── NUMA_NODES.add_range(0, 0x0, 0x1_0000_0000)  // plage 4 GiB
  └── NUMA_GLOBAL_STATS.total_nodes = 1
```

Dans un système NUMA réel, le parseur ACPI SRAT remplace cet appel par
des `register_node()` + `add_range()` multiples avant que la mémoire
système soit disponible.

---

## 6. Allocateur NUMA-aware (`allocator/numa_aware.rs`)

Wrapping du buddy avec stratégie `LocalFirst` par défaut :
```
NumaAllocContext
  ├── policy:         NumaPolicy (LocalFirst | Interleave | Bind | Preferred)
  ├── bind_node:      Option<NumaNode>   — nœud cible pour Bind/Preferred
  └── allow_fallback: bool               — fallback inter-nœuds si épuisé
```

**Conformité IA-KERNEL-01** : les hints NUMA proviennent de
`allocator/ai_hints.rs` (table `.rodata`), aucune inférence runtime.
