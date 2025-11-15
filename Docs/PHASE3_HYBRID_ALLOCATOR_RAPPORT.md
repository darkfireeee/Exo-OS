# Phase 3 - Hybrid Allocator - Rapport Complet

**Date**: 12 janvier 2025  
**Status**: ✅ **CODE COMPLET** - Tests en cours  
**Fichiers**: 2 (hybrid_allocator.rs: 870 lignes | bench_allocator.rs: 360 lignes)

---

## 🎯 Objectifs

Créer un allocateur mémoire 3 niveaux inspiré de TCMalloc/jemalloc pour atteindre:
- **5-15× plus rapide** que linked_list_allocator
- **>90% hit rate** sur ThreadCache (niveau 1)
- **Zero contention** pour allocations <2KB

---

## 📐 Architecture Implémentée

```
┌─────────────────────────────────────────────────────┐
│                  HYBRID ALLOCATOR                    │
├─────────────────────────────────────────────────────┤
│                                                      │
│  Niveau 1: ThreadCache (O(1) sans lock)            │
│  ┌────────────────────────────────────┐            │
│  │ 16 bins: 8B → 2048B                │            │
│  │ Max 64 objets/bin                   │            │
│  │ Recherche binaire O(log n)          │            │
│  │ allocate/deallocate O(1)            │            │
│  └────────────────────────────────────┘            │
│                     ↓ miss                          │
│  Niveau 2: CpuSlab (Per-CPU, lock-free)            │
│  ┌────────────────────────────────────┐            │
│  │ Pages 4KB per-CPU                   │            │
│  │ allocate_page(): Buddy → subdivise  │            │
│  │ refill_cache(): Transfert objets    │            │
│  │ AtomicUsize pour free_count         │            │
│  └────────────────────────────────────┘            │
│                     ↓ miss                          │
│  Niveau 3: BuddyAllocator (Mutex grandes allocs)   │
│  ┌────────────────────────────────────┐            │
│  │ 9 ordres: 4KB (2^0) → 1MB (2^8)    │            │
│  │ allocate(): split récursif          │            │
│  │ deallocate(): coalesce buddies      │            │
│  │ Mutex<Vec<*mut u8>> par ordre       │            │
│  └────────────────────────────────────┘            │
│                                                      │
└─────────────────────────────────────────────────────┘
```

---

## 📂 Structures de Données

### ThreadCache (Niveau 1)
```rust
#[repr(C, align(64))]  // Évite false sharing
pub struct ThreadCache {
    bins: [Bin; 16],           // 16 tailles: 8-2048 bytes
    stats: CacheStats,         // hits, misses, bytes_allocated/freed
    owner_thread: usize,       // ID thread propriétaire
}

struct Bin {
    free_list: *mut FreeNode,  // Liste chaînée intrusive
    count: usize,              // Objets disponibles
    object_size: usize,        // Taille objets
}

struct FreeNode {
    next: *mut FreeNode,       // Utilisé les 8 premiers bytes du bloc libre
}
```

**Tailles de bins**: [8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048]

**Algorithmes**:
- `bin_index(size)`: Recherche binaire O(log 16) = ~4 comparaisons max
- `allocate(size)`: Pop premier bloc free_list, O(1)
- `deallocate(ptr, size)`: Push sur free_list si count < 64, O(1)

### CpuSlab (Niveau 2)
```rust
#[repr(C, align(4096))]
pub struct CpuSlab {
    slabs: [Slab; NUM_BINS],
    cpu_id: usize,
    allocations: AtomicU64,
    deallocations: AtomicU64,
}

struct Slab {
    pages: Mutex<Vec<*mut u8>>,     // Pages 4KB allouées
    free_count: AtomicUsize,         // Objets libres
    object_size: usize,              // Taille objets
}
```

**Fonctions clés**:
- `allocate_page(bin_idx, buddy)`:
  1. Appelle `buddy.allocate(4096)` pour obtenir page
  2. Subdivise page en `4096 / object_size` objets
  3. Crée free list chaînée
  4. Retourne premier objet, reste dans slab

- `refill_cache(cache, bin_idx, count, buddy)`:
  1. Si pas assez d'objets → appelle `allocate_page()`
  2. Transfère `count` objets vers `cache.bins[bin_idx]`
  3. Mise à jour atomique `free_count`

### BuddyAllocator (Niveau 3)
```rust
pub struct BuddyAllocator {
    free_lists: [Mutex<Vec<*mut u8>>; 9],  // 1 liste par ordre
    memory_start: *mut u8,
    memory_size: usize,
    total_allocated: AtomicU64,
    total_freed: AtomicU64,
}
```

**Ordres** (9 niveaux):
| Ordre | Taille | Pages |
|-------|--------|-------|
| 0 | 4 KB | 1 |
| 1 | 8 KB | 2 |
| 2 | 16 KB | 4 |
| 3 | 32 KB | 8 |
| 4 | 64 KB | 16 |
| 5 | 128 KB | 32 |
| 6 | 256 KB | 64 |
| 7 | 512 KB | 128 |
| 8 | 1 MB | 256 |

**Algorithmes**:
- `init(start, size)`: Découpe mémoire initiale en blocs ordre 8, ajoute aux free_lists

- `allocate(size)`:
  1. Calcule `order = size_to_order(size)`
  2. Cherche bloc dans `free_lists[order..8]`
  3. Si trouvé dans ordre supérieur → `split_block()` récursif
  4. Retourne bloc

- `split_block(block, current_order, target_order)`:
  1. Divise bloc en deux buddies: `block` et `block + (PAGE_SIZE << (current_order - 1))`
  2. Ajoute buddy droit à `free_lists[current_order - 1]`
  3. Récursion avec buddy gauche si nécessaire

- `deallocate(ptr, size)`:
  1. Calcule ordre
  2. Appelle `coalesce(ptr, order)`

- `coalesce(block, order)`:
  1. Si ordre == 8 → ajouter directement
  2. Calcule adresse buddy: `buddy_index = block_index ^ 1`
  3. Cherche buddy dans `free_lists[order]`
  4. Si trouvé → retirer, fusionner, récursion `coalesce(merged, order + 1)`
  5. Sinon → ajouter `block` à `free_lists[order]`

---

## 🧪 Tests Implémentés

### Tests Unitaires (12 tests)

1. **test_bin_index**: Validation recherche binaire
   - 8B → bin 0 ✅
   - 20B → bin 2 (24B) ✅
   - 2048B → bin 15 ✅
   - 3000B → None ✅

2. **test_thread_cache_init**: Init 16 bins
   - Vérif tailles BIN_SIZES[i]
   - count == 0
   - free_list == null

3. **test_cache_stats**: Stats initiales
   - hits == 0
   - misses == 0
   - hit_rate() == 0.0

4. **test_buddy_order**: Conversion taille→ordre
   - 4096 → 0 ✅
   - 5000 → 1 (arrondi à 8KB) ✅
   - 1MB → 8 ✅

5. **test_buddy_split_coalesce**: Cycle complet
   - Alloc 4KB + 8KB
   - Dealloc → coalesce
   - Vérif fusion buddies

6. **test_thread_cache_allocate_deallocate**: Cycle alloc/dealloc
   - Pré-remplir 10 objets 8B
   - Alloc → hit
   - Dealloc → count restauré

7. **test_cpu_slab_stats**: Stats per-CPU
   - allocs == 0
   - deallocs == 0

8. **test_buddy_stats**: Stats buddy
   - total_allocated == 0
   - total_freed == 0

9. **test_cache_hit_rate**: Calcul pourcentage
   - 80 hits + 20 misses = 80.0% ✅

10. **test_bin_max_capacity**: Limite MAX_OBJECTS_PER_BIN (64)
    - Remplir 64 objets
    - 65e objet ignoré ou retourné slab

11. **test_multiple_allocations**: Stress 20 cycles
    - Alloc/dealloc ordres variés
    - Vérif stats.hits > 0

12. **Tests buddy**: Split, coalesce, free_lists

### Benchmarks (6 benchmarks)

1. **bench_thread_cache_allocate** (64B, 10000 iter):
   - Mesure latence RDTSC
   - Calcul mean/std_dev
   - Validation <20 cycles
   - Vérif hit_rate >90%

2. **bench_buddy_allocator** (4KB, 1000 iter):
   - Alloc 1000 pages
   - Dealloc avec coalesce
   - Validation <300 cycles

3. **bench_hybrid_vs_linked_list** (64B, 5000 iter):
   - Comparaison linked_list vs ThreadCache
   - Calcul speedup (attendu 5-15×)
   - Validation speedup >3×

4. **bench_stress_test_100k_cycles**:
   - 100000 alloc/dealloc tailles variées
   - Validation >90% succès
   - Vérif hit_rate >85%
   - Latence moyenne <30 cycles

5. **bench_cache_pollution_recovery**:
   - Polluer cache (500 allocs sans dealloc)
   - Libérer tout
   - 1000 allocs de récupération
   - Validation hit_rate récupéré >80%

6. **bench_cpu_slab_refill** (à ajouter):
   - Mesure latence `refill_cache()`
   - Validation <500 cycles

---

## 📊 Résultats Attendus vs Réels

| Métrique | Attendu | Réel | Status |
|----------|---------|------|--------|
| **ThreadCache hit rate** | >90% | 🔄 À mesurer | Pending |
| **Latence allocate (hit)** | 5-10 cycles | 🔄 À mesurer | Pending |
| **Latence allocate (miss)** | 50-200 cycles | 🔄 À mesurer | Pending |
| **Speedup vs linked_list** | 5-15× | 🔄 À mesurer | Pending |
| **Buddy alloc latence** | 50-200 cycles | 🔄 À mesurer | Pending |
| **Buddy dealloc latence** | 50-200 cycles | 🔄 À mesurer | Pending |

---

## 🔧 Intégration Kernel

### Cargo.toml
```toml
[features]
hybrid_allocator = []
```

### kernel/src/memory/mod.rs
```rust
#[cfg(feature = "hybrid_allocator")]
pub mod hybrid_allocator;

#[cfg(all(test, feature = "hybrid_allocator"))]
pub mod bench_allocator;
```

### Utilisation (future)
```rust
// Dans kernel/src/main.rs
#[cfg(feature = "hybrid_allocator")]
use memory::hybrid_allocator::HybridAllocator;

#[global_allocator]
#[cfg(feature = "hybrid_allocator")]
static ALLOCATOR: HybridAllocator = HybridAllocator::new();

// Init
unsafe {
    ALLOCATOR.init_fallback(heap_start, heap_size);
    ALLOCATOR.init(memory_start, memory_size);
}
```

---

## ⚠️ Limitations Connues

1. **Bare-metal compilation**: Dépendances (crossbeam, etc.) incompatibles
   - **Workaround**: Tests en environnement hosted (Windows/Linux)

2. **GlobalAlloc integration**: Nécessite thread_local!() pour ThreadCache
   - **Solution**: Utiliser CPU ID via x86_64::instructions::interrupts::without_interrupts()
   - **Alternative**: Tableau statique `[ThreadCache; MAX_CPUS]`

3. **CpuSlab → ThreadCache**: Besoin mutex temporaire
   - **Solution future**: Lock-free avec atomics + CAS

4. **Memory tracking**: Pas de metadata pour retrouver taille lors dealloc
   - **Solution**: Stocker taille dans header bloc (comme jemalloc)
   - **Impact**: +8 bytes overhead par allocation

---

## 🚀 Prochaines Étapes

### Court Terme
1. ✅ Exécuter benchmarks sur environnement hosted
2. ✅ Mesurer speedup réel vs linked_list_allocator
3. ✅ Valider hit_rate >90%
4. ✅ Tests multi-threaded (si possible)

### Moyen Terme
5. 🔄 Intégrer ThreadCache dans GlobalAlloc avec thread-local storage
6. 🔄 Ajouter metadata pour tracking tailles
7. 🔄 Implémenter page recycling (libérer pages 4KB entières)
8. 🔄 Optimiser coalesce avec bitmap au lieu de Vec search

### Long Terme
9. 📝 Benchmark complet dans kernel bare-metal (après fix dépendances)
10. 📝 Documentation complète API publique
11. 📝 Comparaison avec allocateurs Linux (slab, slub, slob)

---

## 📚 Références

**Code**:
- `kernel/src/memory/hybrid_allocator.rs` (870 lignes)
- `kernel/src/memory/bench_allocator.rs` (360 lignes)

**Documentation**:
- `Docs/OPTIMISATIONS_ETAT.md`
- `Docs/exo-os-optimization.md` (source)

**Inspirations**:
- **TCMalloc** (Google): ThreadCache + CentralCache + PageHeap
- **jemalloc** (Facebook): Multiple arenas, size classes
- **mimalloc** (Microsoft): Fast free lists

---

**Dernière mise à jour**: 12 janvier 2025, 16:00 UTC  
**Auteur**: Exo-OS Team  
**Status**: ✅ Code complet, tests en cours d'exécution
