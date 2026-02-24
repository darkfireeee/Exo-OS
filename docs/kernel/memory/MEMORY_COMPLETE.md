# Module memory/ — Documentation Complète
> Extrait de `kernel/src/memory/**` — tous les sous-modules
>
> **COUCHE 0** — ne dépend de RIEN (scheduler/, process/, ipc/, fs/)
> Toute communication inverse se fait par injection de trait ou pointeur de fonction.

---

## Table des matières

1. [Vue d'ensemble](#1-vue-densemble)
2. [Règles d'architecture](#2-règles-darchitecture)
3. [Ordre d'initialisation](#3-ordre-dinitialisation)
4. [core/ — Types fondamentaux](#4-core--types-fondamentaux)
5. [physical/ — RAM physique](#5-physical--ram-physique)
6. [virtual/ — Espace d'adressage virtuel](#6-virtual--espace-dadressage-virtuel)
7. [heap/ — Allocateur dynamique kernel](#7-heap--allocateur-dynamique-kernel)
8. [dma/ — Moteur DMA](#8-dma--moteur-dma)
9. [swap/ — Backend swap](#9-swap--backend-swap)
10. [cow/ — Copy-on-Write](#10-cow--copy-on-write)
11. [huge_pages/ — Pages géantes](#11-huge_pages--pages-géantes)
12. [protection/ — Protection CPU](#12-protection--protection-cpu)
13. [integrity/ — Intégrité de la pile](#13-integrity--intégrité-de-la-pile)
14. [numa/ — NUMA](#14-numa--numa)
15. [utils/ — Utilitaires](#15-utils--utilitaires)
16. [arch_iface.rs — Interface vers arch/](#16-arch_ifacers--interface-vers-arch)

---

## 1. Vue d'ensemble

```
memory/
├── mod.rs           API publique : alloc, map, free
├── arch_iface.rs    Pont memory/ ↔ arch/ (côté memory)
├── core/            Types fondamentaux (PhysAddr, Frame, PageFlags…)
├── physical/        RAM physique (buddy, slab, zones, frames, NUMA)
├── virtual/         Espace virtuel (page tables, VMA, fault handler)
├── heap/            Allocateur global kernel (#[global_allocator])
├── dma/             Canaux DMA, IOMMU, completion
├── swap/            Backend swap, politique LRU/CLOCK
├── cow/             Tracking CoW lock-free
├── huge_pages/      THP 2 MiB + HugeTLB 1 GiB
├── protection/      NX, SMEP, SMAP, PKU
├── integrity/       Canaries, guard pages, KASAN-lite
├── numa/            (façade → physical/numa/, voir NUMA.md)
└── utils/           FutexTable, OOM killer, shrinker
```

Espace d'adressage x86_64 (4 niveaux, 48 bits) :
```
0x0000_0000_0000_0000 – 0x0000_7FFF_FFFF_FFFF   Espace utilisateur  (128 TiB)
0x0000_8000_0000_0000 – 0xFFFF_7FFF_FFFF_FFFF   Zone non canonique  (trou)
0xFFFF_8000_0000_0000 – 0xFFFF_BFFF_FFFF_FFFF   PHYS_MAP_BASE       (physmap, 64 TiB)
0xFFFF_C000_0000_0000 – 0xFFFF_C7FF_FFFF_FFFF   VMALLOC_BASE        (8 TiB)
0xFFFF_C800_0000_0000 – 0xFFFF_CFFF_FFFF_FFFF   KERNEL_HEAP_START   (heap kernel)
0xFFFF_FFFF_8000_0000 – 0xFFFF_FFFF_FFFF_FFFF   Code + données kernel statiques
```

---

## 2. Règles d'architecture

| Règle | Description |
|-------|-------------|
| **COUCHE 0** | memory/ n'importe jamais scheduler/, process/, ipc/, fs/. |
| **INVERSION DEP** | Toute communication ascendante passe par un trait ou fn pointer injecté au boot. |
| **IA-KERNEL-01** | Tables `.rodata` statiques uniquement — zéro inférence runtime. |
| **EMERGENCY-01** | `EmergencyPool` initialisé EN PREMIER avant tout allocateur. |
| **FUTEX-01** | `FutexTable` = singleton unique — indexé par adresse physique. |
| **LOCK ORDER** | IPC < Scheduler < **Memory** < FS (jamais lock N si on tient N+1). |
| **MEM-04** | `free_pages()` jamais appelé avant TLB shootdown complet. |
| **NO-ALLOC ISR** | Aucun alloc/Box/Vec dans les ISR et handlers critiques. |
| **UNSAFE CONTRACT** | Tout `unsafe { }` précédé d'un `// SAFETY:` obligatoire. |

---

## 3. Ordre d'initialisation

```
Phase 1  — physical/ : EmergencyPool → bitmap → buddy → SLUB → per-CPU pools → NUMA
Phase 2  — virtual/  : KERNEL_AS.init() (via arch/boot/memory_map.rs → arch_iface)
Phase 3  — heap/     : GlobalAllocator statique (actif dès Phase 1 via SLUB)
Phase 4  — dma/      : IOMMU init → channel manager → engines
Phase 5  — protection/ : NX → SMEP → SMAP → PKU
Phase 6  — integrity/ : canary → guard pages → KASAN-lite
Phase 7  — utils/    : futex_table → OOM killer → shrinker registry
Phase 8  — numa/     : registre nœuds NUMA via ACPI SRAT (appel à physical/numa/)
```

---

## 4. core/ — Types fondamentaux

Dépendances : **aucune**. Tous les types sont des constantes de compilation vérifiées statiquement.

### 4.1 `constants.rs`
| Constante | Valeur | Description |
|-----------|--------|-------------|
| `PAGE_SIZE` | 4096 | Taille d'une page standard |
| `PAGE_SHIFT` | 12 | Décalage page |
| `HUGE_PAGE_SIZE` | 2 MiB | Page géante |
| `BUDDY_MAX_ORDER` | 11 | Ordre max buddy (2^11 × 4 KiB = 8 MiB) |
| `CACHE_LINE_SIZE` | 64 | Cache line x86_64 |
| `DMA_RING_SIZE` | 512 | Entrées par anneau DMA |
| `FUTEX_HASH_BUCKETS` | 1024 | Buckets de la FutexTable |
| `PER_CPU_POOL_SIZE` | 512 | Frames par pool per-CPU |
| `EMERGENCY_POOL_SIZE` | 256 | Frames de réserve urgence |
| `MAX_CPUS` | 256 | CPUs maximum |
| `ZONE_DMA_END` | 16 MiB | Limite zone DMA |
| `ZONE_DMA32_END` | 4 GiB | Limite zone DMA32 |

### 4.2 `types.rs`
Types opaques garantissant la canonicité et l'alignement :

| Type | repr | Description |
|------|------|-------------|
| `PhysAddr(u64)` | transparent | Adresse physique — non déréférençable directement |
| `VirtAddr(u64)` | transparent | Adresse virtuelle canonique 48 bits |
| `Frame` | C | Frame physique : `{start: PhysAddr, _pad}` |
| `Page` | C | Page virtuelle : `{start: VirtAddr}` |
| `FrameRange` | C | Plage de frames `[start, end)` |
| `PageRange` | C | Plage de pages `[start, end)` |
| `PhysRange` | C | Plage physique brute `{start: u64, end: u64}` |
| `PageFlags` | transparent | Bits de permissions page (RWX, USER, NX, HUGE…) |
| `AllocFlags` | C | Drapeaux d'allocation (KERNEL, USER, DMA, ZEROED…) |
| `ZoneType` | u8 | DMA / DMA32 / NORMAL / HIGH / MOVABLE |
| `AllocError` | C | OutOfMemory / NotAligned / InvalidSize / ZoneExhausted |

### 4.3 `address.rs`
Translations physique ↔ virtuel via la physmap directe :
```rust
phys_to_virt(phys: PhysAddr) -> VirtAddr        // phys + PHYS_MAP_BASE
virt_to_phys_physmap(virt: VirtAddr) -> PhysAddr // virt − PHYS_MAP_BASE
is_physmap(virt: VirtAddr) -> bool
is_heap(virt: VirtAddr) -> bool
align_up / align_down / is_aligned
```

### 4.4 `layout.rs`
Carte mémoire statique noyau (`const` de type `VirtAddr`) :

| Constante | Description |
|-----------|-------------|
| `PHYS_MAP_BASE` | Base de la physmap directe (64 TiB) |
| `VMALLOC_BASE` | Base de l'espace vmalloc |
| `KERNEL_HEAP_START` | Début du heap kernel |
| `KERNEL_START` | Début du code kernel |
| `USER_END` | Limite haute de l'espace utilisateur |
| `USER_STACK_TOP` | Sommet de pile utilisateur par défaut |
| `KERNEL_PHYS_OFFSET` | Offset physique du kernel (chargé à 1 MiB) |

---

## 5. physical/ — RAM physique

### 5.1 `allocator/`

#### `buddy.rs` — Buddy allocator O(log n)
Allocateur principal de frames physiques.

**Principe** :
- RAM divisée en blocs de `2^order × PAGE_SIZE`.
- Chaque ordre dispose d'une free-list doublement chaînée.
- Allocation : remonte les ordres jusqu'au premier bloc disponible, découpe.
- Libération : recherche du buddy (`P ⊕ (1 << order)`) et fusion récursive.

**Invariants** :
- Bloc d'ordre k = adresse alignée sur `2^k` pages.
- Jamais de fragmentation externe au-delà de 1 bloc par ordre.

**API** :
```rust
pub fn alloc_pages(order: u32, flags: AllocFlags) -> Result<Frame, AllocError>
pub fn free_pages(frame: Frame, order: u32) -> Result<(), AllocError>
pub fn alloc_page(flags: AllocFlags) -> Result<Frame, AllocError>   // order=0
pub fn free_page(frame: Frame) -> Result<(), AllocError>
```

#### `slab.rs` — Slab allocator (petits objets)
Inspiré de Bonwick (1994), adapté `no_std`.

- Classes de taille : 8, 16, 32, 64, 128, 256, 512, 1024, 2048 octets.
- Header slab (32 octets, cacheline-aligned) : `freelist`, `inuse`, `total`.
- 3 listes par cache : `free_list`, `partial_list`, `full_list`.
- Provider trait `SlabPageProvider` — injecte les pages depuis le buddy.

#### `slub.rs` — SLUB (variante optimisée)
Fragmentation réduite par rapport au slab classique :
- Slabs sur des pages complètes.
- Pas de coloration de cache.
- Une seule liste de slabs partiels par cache.
- Header SLUB compact (32 octets).

#### `bitmap.rs` — Bitmap bootstrap
Utilisé **uniquement** pendant la phase d'initialisation (avant que le
buddy soit opérationnel).
- Couvre jusqu'à 512 MiB (131072 frames).
- Tableau statique en `.bss`, jamais désalloué.
- Remplacé par le buddy à la fin de Phase 1.

#### `numa_aware.rs` — Allocateur NUMA-aware
Wrapping du buddy avec politique locale en défaut :

```
NumaAllocContext
  policy:         LocalFirst | Interleave | Bind | Preferred
  bind_node:      Option<NumaNode>   — nœud cible pour Bind/Preferred
  allow_fallback: bool               — fallback inter-nœuds si épuisé
```

#### `ai_hints.rs` — Hints IA statiques
**Règle IA-KERNEL-01 : ZERO inférence runtime.**
Tables lookup compilées (`.rodata`) fournissant des hints d'affinité NUMA
pour l'allocateur. Jamais mises à jour au runtime.

---

### 5.2 `frame/`

#### `descriptor.rs` — FrameDesc
Un `FrameDesc` par frame physique dans la table globale `FRAME_TABLE`.

```
FrameDesc (repr C, align 64)
  flags:      FrameFlags(u16)   — état (FREE, USED, PINNED, DMA, ACCESSED…)
  refcount:   AtomicU32         — compteur CoW (0=libre, 1=exclusif, 2+=partagé)
  zone:       ZoneType(u8)      — DMA/DMA32/NORMAL/HIGH/MOVABLE
  numa_node:  u8                — nœud NUMA propriétaire
  order:      u8                — ordre buddy actuel
  _pad        …
```

#### `ref_count.rs` — Refcount CoW
Compteur de références atomique pour scénarios de partage mémoire
(`fork()`, `mmap()`, SHM) :
- `0` = frame libre
- `1` = exclusif (lecture/écriture)
- `2+` = partagé CoW (lecture seule, copie à l'écriture)

#### `pool.rs` — Per-CPU pools + EmergencyPool
- **Per-CPU** : 512 frames par CPU, lock-free (évite la contention
  sur le buddy global). Drain automatique si > `DRAIN_THRESHOLD`,
  refill si < `REFILL_THRESHOLD`.
- **EmergencyPool** : 256 frames réservés — initialisés EN PREMIER
  (RÈGLE EMERGENCY-01). Utilisés quand toutes les zones sont épuisées.

#### `reclaim.rs` — CLOCK-pro simplifié
Algorithme de récupération de frames pour répondre à la pression mémoire :
1. Scanner la liste `inactive` depuis le pointeur d'horloge.
2. Frame `ACCESSED` → promouvoir en `active`, clear bit.
3. Frame non `ACCESSED` → candidat swap ou libération directe.
4. Frame `PINNED` / `DMA` → sauter.
5. Quand `active` déborde (> `HIGH_WATER_ACTIVE`), dégrader en `inactive`.

Indicateur per-CPU `PF_MEMALLOC` : signale qu'un thread est déjà en
train de reclaimer (évite la récursion).

---

### 5.3 `zone/`

| Fichier | Zone | Plage physique | Usage |
|---------|------|---------------|-------|
| `dma.rs` | DMA | `[0, 16 MiB)` | Devices legacy ISA / vieux PCI |
| `dma32.rs` | DMA32 | `[16 MiB, 4 GiB)` | Devices 32 bits sans IOMMU |
| `normal.rs` | NORMAL | `[4 GiB, ∞)` | Allocations noyau ordinaires |
| `high.rs` | HIGH | `> 896 MiB` | Spécifique 32 bits — inutilisé en 64 bits |
| `movable.rs` | MOVABLE | configurable | Pages migratables (défrag huge pages) |

Chaque zone expose un `ZoneDescriptor` avec `phys_start`, `phys_end`,
`total_frames`, `free_frames (AtomicUsize)`, `numa_node`.

---

### 5.4 `numa/`

Voir [NUMA.md](NUMA.md) pour la documentation détaillée.

Résumé : `node.rs` / `distance.rs` / `policy.rs` / `migration.rs` dans
`physical/numa/`. Le répertoire `memory/numa/` est une façade de compat
qui réexporte tout.

---

## 6. virtual/ — Espace d'adressage virtuel

> Note : le répertoire `virtual/` est déclaré comme module `virt` dans Rust
> (`#[path = "virtual/mod.rs"] pub mod virt`) car `virtual` est un mot-clé réservé.

### 6.1 `address_space/`

#### `kernel.rs` — KernelAddressSpace
Singleton `KERNEL_AS` partagé par tous les processus (moitié haute de
la PML4, indices 256–511). Créé une seule fois, jamais détruit.

Opérations :
```rust
KERNEL_AS.map(virt, frame, flags)   -> Result<(), AllocError>
KERNEL_AS.unmap(virt)               -> Result<Frame, AllocError>
KERNEL_AS.translate(virt)           -> Option<(PhysAddr, PageFlags)>
KERNEL_AS.init(pml4_phys)           // appelé par arch/boot
```

#### `user.rs` — UserAddressSpace
Un par processus. Moitié basse de la PML4 (indices 0–255).

- Contient un `VmaTree` (arbre AVL des zones mappées).
- Gère `clone()` pour `fork()` (duplique les VMAs en CoW).
- Implémente `MigrationPageTableOps` pour `memory/physical/numa/migration.rs`.

#### `mapper.rs` — Mapper
Interface unifiée `map / unmap / remap` appelée par la couche supérieure.
Dispatch vers KernelAS ou UserAS selon l'adresse.

#### `tlb.rs` — TLB management
```rust
flush_single(virt: VirtAddr)          // INVLPG local
flush_range(start, end)               // boucle INVLPG
flush_all()                           // CR3 reload
request_ipi_shootdown(flush: TlbFlushType)  // IPI 0xF2 → tous CPUs actifs
```

Vecteur IPI TLB shootdown : `0xF2` (défini dans `arch_iface.rs`).

---

### 6.2 `page_table/`

#### `x86_64.rs` — Tables 4 niveaux
```
PML4 (512 entrées × 8 octets = 4 KiB)
 └─ PDPT (512 × 8)
     └─ PD (512 × 8)
         └─ PT (512 × 8)
```
Chaque `PageTableEntry` (u64) : bits NX, HUGE, USER, WRITABLE, PRESENT +
adresse physique sur 40 bits.

Primitives ASM : `read_cr3() -> PhysAddr`, `write_cr3(phys)`, `invlpg(virt)`.

#### `walker.rs` — Page table walker
Parcours récursif PML4 → PT avec allocation à la demande ou mode read-only.

```rust
pub fn walk(root: PhysAddr, virt: VirtAddr, opts: WalkOpts) -> WalkResult
// WalkResult : Found(PhysAddr, PageFlags) | NotMapped | HugePage(PhysAddr)
```

#### `builder.rs` — Constructeur progressif
Construit les page tables initiales du kernel (boot, avant `KERNEL_AS`).
Mapping progressif incrémental.

#### `kpti_split.rs` — KPTI
Tables scindées `user_pt` / `kernel_pt` :
- `user_pt` : seulement les stubs d'entrée syscall/interrupt mappés.
- `kernel_pt` : tables complètes (actives uniquement en Ring 0).
- Switch CR3 effectué dans l'ASM de bas niveau (`arch/x86_64/boot/trampoline_asm.rs`).

---

### 6.3 `vma/`

#### `descriptor.rs` — VmaDescriptor
```
VmaDescriptor
  start:      VirtAddr
  end:        VirtAddr
  flags:      VmaFlags      (READ | WRITE | EXEC | SHARED | GROWSDOWN)
  page_flags: PageFlags
  backing:    VmaBacking    (Anonymous | File {inode_id, offset} | Device)
```

#### `tree.rs` — Arbre AVL des VMAs
- Recherche d'une VMA par adresse : O(log n).
- `MAX_VMAS_PER_PROCESS = 65536`.
- Insertion / suppression avec rééquilibrage AVL.

#### `operations.rs` — mmap / munmap / mprotect
```rust
do_mmap(as, params: VmaAllocParams)    -> Result<VirtAddr, AllocError>
do_munmap(as, start, size)             -> Result<(), AllocError>
do_mprotect(as, start, size, flags)    -> Result<(), AllocError>
do_mremap(as, old_start, old_size, new_size, flags)
```

#### `cow.rs` — VMA CoW
Duplication en lecture lors de `fork()`. La VMA devient `SHARED + READ_ONLY`.
À la première écriture → page fault CoW → `memory/cow/breaker.rs`.

---

### 6.4 `fault/`

#### `handler.rs` — Page fault dispatcher
Dispatcher du `#PF` (vecteur 14) vers :

| Cause | Handler | Description |
|-------|---------|-------------|
| Page absente, VMA anonyme | `demand_paging.rs` | Alloue + mappe frame |
| Page absente, VMA fichier | `demand_paging.rs` | Charge depuis backing store |
| Écriture sur page CoW | `cow.rs` | Copie physique de la frame |
| Page swappée | `swap_in.rs` | Lit depuis swap + restaure |

Statistiques atomiques : `total`, `demand_paging`, `cow_breaks`,
`swap_ins`, `permission_faults`.

#### `demand_paging.rs`
Mappe une frame physique fraîche (ou lue depuis le backing store) à
la première faute d'accès.

#### `cow.rs`
Réalise la copie physique à l'écriture :
1. Vérifie `refcount > 1`.
2. Alloue une nouvelle frame.
3. Copie le contenu via physmap.
4. Remplace la PTE par la nouvelle frame.
5. Décrémente le refcount CoW.

#### `swap_in.rs`
Lit une page depuis le backend swap et la ré-insère dans la page table.

---

## 7. heap/ — Allocateur dynamique kernel

### 7.1 `allocator/`

#### `hybrid.rs` — Dispatch par taille
Interface unique du heap kernel :
- `size <= HEAP_LARGE_THRESHOLD (2 KiB)` → SLUB (per-CPU magazine).
- `size > 2 KiB` → vmalloc (grandes allocations non-contiguës).

#### `size_classes.rs`
Classes heap : 8, 16, 32, 64, 128, 256, 512, 1024, 2048 octets + large.

#### `global.rs` — `#[global_allocator]`
Implémentation de `GlobalAlloc` pour Rust. `alloc()` → `hybrid.rs`.
Actif dès la Phase 1 (SLUB disponible).

---

### 7.2 `thread_local/`

#### `cache.rs` — TLS per-CPU
Hot path alloc/free entièrement sans lock.
- Une paire de magazines par classe de taille et par CPU.
- Alloc : dépile du magazine chaud.
- Free : empile sur le magazine froid.
- Drain/refill automatique vers SLUB quand magazine plein/vide.
- Latence cible : **< 25 cycles** en alloc sur le hot path.

#### `magazine.rs` — Magazine layer
Batch d'allocations/libérations (64 pointeurs par magazine).
Réduit la contention sur SLUB global.

#### `drain.rs` — Drain vers pool global
Vide un magazine TLS vers SLUB quand le thread se termine.

---

### 7.3 `large/`

#### `vmalloc.rs` — Grandes allocations
Pour les allocations `> 2 KiB` dans l'espace kernel :

- Espace `VMALLOC_BASE → VMALLOC_BASE + VMALLOC_SIZE`.
- Chaque allocation préfixée d'un `VmallocHeader` (64 octets, cacheline-aligned).
- Alloue des frames physiques via buddy ; les mappe dans `KERNEL_AS`.
- Les frames ne sont **pas** physiquement contiguës entre elles.
- Libération : démappe + `free_pages()` frame par frame.

---

## 8. dma/ — Moteur DMA

> ⚠️ `dma/completion/wakeup.rs` appelle `process/` via le trait `DmaWakeupHandler`
> (inversion de dépendance) — zéro import direct de process/.

### 8.1 `core/`

| Fichier | Contenu |
|---------|---------|
| `types.rs` | `DmaChannelId`, `IommuDomainId`, `DmaTransactionId`, `DmaAddr`, `DmaBuf`, `DmaDesc`, `DmaChannel` |
| `descriptor.rs` | `DmaRing` — anneau de 512 descripteurs, page-aligned |
| `mapping.rs` | DMA coherent / streaming mappings (physmap → IOMMU) |
| `error.rs` | `DmaError` : `OutOfChannels`, `IommuFault`, `Timeout`, `InvalidAddress` |

### 8.2 `iommu/`

| Fichier | Description |
|---------|-------------|
| `intel_vtd.rs` | Intel VT-d (DMAR) — tables IOMMU 4 niveaux |
| `amd_iommu.rs` | AMD-Vi — `PRIVABRT_EN` flag (corrigé cette session) |
| `arm_smmu.rs` | ARM SMMU (placeholder ARM64 futur) |
| `domain.rs` | Domaine IOMMU : isolation par device |
| `page_table.rs` | Tables de pages IOMMU 4 niveaux |

### 8.3 `channels/`

| Fichier | Description |
|---------|-------------|
| `manager.rs` | Pool de canaux DMA (allocation/libération) |
| `channel.rs` | Canal : ring producer/consumer + état |
| `priority.rs` | RT (temps réel) vs best-effort |
| `affinity.rs` | Canal ↔ CPU NUMA affinity (localité) |

### 8.4 `engines/`

| Fichier | Device |
|---------|--------|
| `ioat.rs` | Intel IOAT DMA Engine |
| `idxd.rs` | Intel DSA (Data Streaming Accelerator) |
| `ahci_dma.rs` | AHCI / SATA |
| `nvme_dma.rs` | NVMe PCIe natif |
| `virtio_dma.rs` | VirtIO (VM) |

### 8.5 `ops/`

| Fichier | Opération |
|---------|-----------|
| `memcpy.rs` | DMA memcpy device ↔ RAM |
| `memset.rs` | DMA memset |
| `scatter_gather.rs` | Scatter-Gather lists |
| `cyclic.rs` | DMA cyclique (audio / streaming) |
| `interleaved.rs` | RAID-like interleaved |

### 8.6 `completion/`

#### `handler.rs`
Gère jusqu'à `MAX_PENDING_COMPLETIONS = 512` transactions en attente.
RTT de completion cible : **< 500 ns** depuis l'IRQ.

**Pattern de réveil (inversion de dépendance)** :
```
IRQ DMA → handler.rs → wake_on_completion(txid)
                     → DmaWakeupHandler::wake(txid)   ← trait
                     → process::wakeup_thread()       ← implémenté par process/
```

#### `polling.rs` — Polling haute fréquence
Mode alternatif à l'IRQ pour les scénarios très faible latence (DPDK-like).

#### `wakeup.rs`
Enregistre un `DmaWakeupHandler` au boot. Jamais `process::wakeup_thread()`
appelé directement depuis memory/.

### 8.7 `stats/counters.rs`
Compteurs DMA atomiques : throughput (octets/s), latence (ns), erreurs.

---

## 9. swap/ — Backend swap

### `backend.rs`
Abstraction du dispositif de swap (partition ou fichier) :

```rust
pub trait SwapDevice {
    fn write_page(&self, slot: SwapSlot, data: *const u8) -> Result<(), SwapError>;
    fn read_page(&self, slot: SwapSlot, buf: *mut u8)     -> Result<(), SwapError>;
    fn free_slot(&self, slot: SwapSlot);
    fn alloc_slot(&self) -> Option<SwapSlot>;
    fn device_name(&self) -> &str;
}
```

Intégration FS via trait — jamais d'import direct `fs/`.

### `policy.rs` — Politique d'éviction CLOCK
Implémente l'algorithme CLOCK (approximation LRU) :
- `add_to_inactive(frame)` — nouveau candidat.
- `clock_scan(target)` — libère `target` frames.
- `promote_to_active(frame)` — frame récemment accédé.
- Déclenchement de l'OOM killer si `is_critical()`.

### `compress.rs` — Compression en RAM (zswap)
Compresse les pages before le swap sur disque (LZ4 / ZSTD) :
- `ZSWAP_SLOTS` : table statique de slots compressés.
- `zswap_store(frame)` → compresse + stocke en RAM.
- `zswap_load(slot)` → décompresse + retourne frame.
- Réduit la latence swap d'un facteur ×5–10.

### `cluster.rs` — Regroupement I/O
Regroupe les écritures swap consécutives en une seule I/O vectorielle
(readahead / write clustering).

---

## 10. cow/ — Copy-on-Write

### `tracker.rs`
Table de hachage `COW_TABLE_SIZE = 4096` entrées.
- Clé : numéro de frame.
- Valeur : refcount CoW (AtomicU32).
- Lock-free : CAS sur `frame_idx` pour insertion/suppression.

### `breaker.rs`
Réalise la rupture CoW lors d'une faute d'écriture :
1. `cow_tracker::dec_ref(frame)`.
2. Si `refcount == 1` → la frame est maintenant exclusive → pas de copie.
3. Si `refcount > 1` → `alloc_page()` + `memcpy` physmap + update PTE.
4. `let _ = free_page(old_frame)` — libère l'ancienne frame.

---

## 11. huge_pages/ — Pages géantes

### `thp.rs` — Transparent Huge Pages (2 MiB)
```
HUGE_PAGE_ORDER = 9   (2^9 × 4 KiB = 2 MiB)

ThpMode : Disabled | Madvise | Always
```
- `try_alloc_huge(virt, flags)` — alloue une frame d'ordre 9 via buddy.
- `collapse_region(as, virt)` — fusionne 512 pages 4 KiB en une THP.
- Statistiques : `thp_allocated`, `thp_collapsed`, `thp_split`.

### `hugetlbfs.rs` — HugeTLBfs (1 GiB)
Pages gigantesques pré-allouées au boot, réservées dans un pool dédié.
- `HUGETLB_ORDER = 18` (2^18 × 4 KiB = 1 GiB).
- Accès via `HUGETLB_POOL` statique.
- `let _ = free_pages()` pour la libération (résultat ignoré, réintégré dans le pool).

### `split.rs`
Découpe une THP (2 MiB) en 512 pages 4 KiB :
1. Alloue 512 frames d'ordre 0.
2. Copie le contenu via physmap.
3. Met à jour les 512 PTEs.
4. Libère la THP d'ordre 9.

---

## 12. protection/ — Protection CPU

| Fichier | Mécanisme | Bit CPU | Effet |
|---------|-----------|---------|-------|
| `nx.rs` | NX / XD bit | PT.63 | Interdit l'exécution sur les pages données |
| `smep.rs` | SMEP | CR4.20 | Interdit au kernel d'exécuter du code user |
| `smap.rs` | SMAP | CR4.21 | Interdit au kernel d'accéder à la mémoire user (sauf fenêtre STAC/CLAC) |
| `pku.rs` | PKU | CR4.22 + PKRU | Protection Keys : 16 domaines de permission par page |

### `smap.rs` — Pattern d'utilisation
```rust
// Ouvrir une fenêtre d'accès user (STAC)
let _guard = unsafe { SmapAccessGuard::new() };  // active AC (STAC)
// Accès à la mémoire user ici
// _guard droppé → CLAC automatique (RAII)
```

### `smep.rs`
```rust
SmepGuard::disable() -> SmepRestoreGuard  // désactive SMEP si nécessaire (rare)
// RAII : réactive SMEP au drop
```

### `pku.rs`
```rust
PkuKey : 0..15   // 16 clés de protection
PKRU   : registre per-CPU, mis à jour via WRPKRU
PkuGuard::new(key, perm) -> PkuRestoreGuard  // RAII
```

---

## 13. integrity/ — Intégrité de la pile

### `canary.rs` — Stack canaries
- Init : `RDTSC ⊕ XOR` → valeur pseudo-aléatoire par CPU.
- Stockée dans `CANARY_TABLE[cpu_id]` (lecture seule après init).
- **Deux niveaux** :
  1. `cpu_canary` : valeur CPU, commune à tous les threads du CPU.
  2. `thread_canary` : `cpu_canary ⊕ tid` — stocké dans le TCB.
- Violation → `canary_violation_handler()` (kernel panic).

### `guard_pages.rs` — Guard pages
Pages mappées sans présence (`PRESENT=0`) en bordure de pile/heap :
- Toute écriture → `#PF` → détection d'overflow ou d'underflow.
- Configurées par `install_guard_pages(addr, n_pages)`.

### `sanitizer.rs` — KASAN-lite (debug)
Détection d'accès mémoire invalides en build debug :
- Shadow memory : 1 octet de shadow pour 8 octets de heap.
- Instrumentation des allocs/frees pour rouge-peindre les zones libres.
- Activé uniquement si `cfg!(debug_assertions)`.

---

## 14. numa/ — NUMA

Façade de rexports vers `physical/numa/`. Voir [NUMA.md](NUMA.md) pour
la documentation complète (structures, algorithmes, problème UB résolu).

```rust
// memory/numa/mod.rs — re-exports uniquement
pub use crate::memory::physical::numa::{
    NumaNode, NumaNodeTable, NUMA_NODES, NumaPolicy, NumaNodeMask,
    NumaDistanceTable, NUMA_DISTANCE, migrate_page, migrate_pages_batch,
    ...
};
pub unsafe fn init() { crate::memory::physical::numa::init(); }
```

---

## 15. utils/ — Utilitaires

### `futex_table.rs` — FutexTable (SINGLETON UNIQUE)
**Règle FUTEX-01 : une seule FutexTable, indexée par adresse physique.**

```
FutexTable
  buckets: [FutexBucket; FUTEX_HASH_BUCKETS]   (1024 buckets)
  FutexBucket
    inner: spin::Mutex<BucketInner>
    BucketInner
      waiters: [FutexWaiter; MAX_WAITERS_PER_BUCKET]
```

- Clé de hash : `phys_addr & (FUTEX_HASH_BUCKETS - 1)`.
- Adresse **physique** (pas virtuelle) = partage inter-processus via mémoire partagée.
- `FUTEX_LOCK_PI` : double verrouillage des buckets lo → hi (prévention deadlock
  par ordre de hash, fix `addr_of!` appliqué cette session).

### `oom_killer.rs` — OOM killer
Architecture inversion dep :
- Déclenché par `swap/policy.rs` ou buddy quand épuisé.
- Sélectionne la victime via `OomScorer` trait (implémenté par process/).
- Signale la mort via `OOM_KILL_SENDER : OomKillSendFn` (fn pointer enregistré au boot).
- Accède à `EmergencyPool` pour les allocations post-OOM.

### `shrinker.rs` — MemoryShrinker trait
Mécanisme de pression mémoire descendante :
```rust
pub trait MemoryShrinker {
    fn shrink(&self, target_pages: usize) -> usize;  // retourne pages libérées
    fn priority(&self) -> u8;
}
pub fn register_shrinker(s: &'static dyn MemoryShrinker);
pub fn run_shrinkers(target: usize) -> usize;
```
`fs/` et `ipc/` s'enregistrent au boot — memory/ ne les connaît pas.

---

## 16. `arch_iface.rs` — Interface vers arch/

Côté memory/ du pont bidirectionnel avec arch/ :

**Constantes d'intégration** :
```rust
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = 0xF2;  // doit correspondre à arch/idt
pub const IPI_RESCHEDULE_VECTOR: u8    = 0xF1;
```

**Flux d'initialisation** :
```
arch/x86_64/boot/memory_map.rs
  → memory::arch_iface::init_from_regions(regions, count)
      Phase 1 : EmergencyPool::init()          ← RÈGLE EMERGENCY-01
      Phase 2 : bitmap::init(start, end)
      Phase 3 : buddy::init_from_regions()
      Phase 4 : numa::init()
```

**Flux TLB** :
```
arch/x86_64/memory_iface.rs
  → memory::virt::address_space::tlb::register_tlb_ipi_sender(fn_ptr)
     └─ stocké comme fn pointer, appelé par tlb.rs lors des shootdowns
```

**Règles** :
- `MEM-01` : memory/ est COUCHE 0 — ne dépend PAS de scheduler/, process/…
- `MEM-02` : EmergencyPool EN PREMIER.
- `MEM-04` : `free_pages()` jamais avant TLB shootdown complet.

---

## Annexe — Dépendances inter-modules (intra memory/)

```
core/          ← dépendance de: TOUS
physical/      ← core/
virtual/       ← core/ + physical/
heap/          ← core/ + physical/ + virtual/
dma/           ← core/ + physical/ + virtual/
swap/          ← core/ + physical/
cow/           ← core/ + physical/
huge_pages/    ← core/ + physical/ + virtual/
protection/    ← core/
integrity/     ← core/
numa/          ← façade → physical/numa/
utils/         ← core/ + physical/
arch_iface/    ← core/ + physical/ + virtual/
```

**Aucun module de memory/ ne dépend de scheduler/, process/, ipc/, fs/.**
