# 📋 DOC 2 — MODULE MEMORY/ : CONCEPTION COMPLÈTE
> Exo-OS · Couche 0 Absolue · Aucune dépendance externe
> Règles anti-crash · anti-deadlock · anti-corruption · anti-race

---

## POSITION DANS L'ARCHITECTURE

```
┌─────────────────────────────────────────────────────────┐
│  memory/  ← COUCHE 0 ABSOLUE                            │
│                                                         │
│  DÉPEND DE : RIEN (zéro import d'autres modules kernel) │
│  EST APPELÉ PAR : scheduler/, ipc/, fs/, process/       │  ← ✅ CORRIGÉ: "dma/" retiré
│  DÉPENDANCES AUTORISÉES : arch/ (pour instructions ASM) │  #   dma/ EST dans memory/, pas appelant de memory/
│                           core:: (bibliothèque Rust)     │
└─────────────────────────────────────────────────────────┘
```

**RÈGLE ABSOLUE :** Si un fichier dans `memory/` contient
`use crate::scheduler`, `use crate::ipc`, `use crate::fs`,
`use crate::process` → **BUG ARCHITECTURAL IMMÉDIAT**.

---

## ARBORESCENCE COMPLÈTE

```
kernel/src/memory/
├── mod.rs                              # API publique exportée
│
├── core/                               # Types fondamentaux (zéro logique)
│   ├── mod.rs
│   ├── types.rs                        # PhysAddr, VirtAddr, Page, Frame, PageFlags
│   ├── address.rs                      # Translations, alignements, assertions
│   ├── layout.rs                       # Carte mémoire statique (KERNEL_START...)
│   └── constants.rs                    # PAGE_SIZE, HUGE_PAGE, CACHE_LINE, etc.
│
├── physical/                           # Gestion RAM physique
│   ├── mod.rs
│   ├── allocator/
│   │   ├── mod.rs
│   │   ├── buddy.rs                    # Buddy allocator principal O(log n)
│   │   ├── slab.rs                     # Slab (objets fixes, cache-friendly)
│   │   ├── slub.rs                     # SLUB (slab amélioré, moins de fragmentation)
│   │   ├── bitmap.rs                   # Bitmap (bootstrap uniquement)
│   │   ├── ai_hints.rs                 # Hints NUMA statiques — lookup table .rodata
│   │   │                               # ⚠️ Zéro inférence runtime, lecture seule
│   │   └── numa_aware.rs               # Policy NUMA (local-first, interleaved, bind)
│   ├── frame/
│   │   ├── mod.rs
│   │   ├── descriptor.rs               # FrameDesc: flags, refcount, zone, numa_node
│   │   ├── ref_count.rs                # Atomic refcount CoW
│   │   ├── pool.rs                     # Per-CPU pools (512 frames, lock-free)
│   │   └── emergency_pool.rs           # EmergencyPool statique (64 WaitNode, init EN PREMIER)
│   └── zone/
│       ├── mod.rs
│       ├── dma.rs                      # Zone DMA (<16 MB)
│       ├── dma32.rs                    # Zone DMA32 (<4 GB)
│       ├── normal.rs                   # Zone NORMAL
│       ├── high.rs                     # Zone HIGH (32-bit uniquement)
│       └── movable.rs                  # Zone MOVABLE (défrag huge pages)
│
├── virtual/                            # Espace d'adressage virtuel
│   ├── mod.rs
│   ├── address_space/
│   │   ├── mod.rs
│   │   ├── kernel.rs                   # Kernel address space (global, fixe)
│   │   ├── user.rs                     # User address space (par processus)
│   │   ├── mapper.rs                   # map/unmap/remap interface unifiée
│   │   └── tlb.rs                      # TLB flush local + IPI shootdown synchrone
│   ├── page_table/
│   │   ├── mod.rs
│   │   ├── x86_64.rs                   # PML4→PDPT→PD→PT (4 niveaux)
│   │   ├── walker.rs                   # Page table walker
│   │   ├── builder.rs                  # Constructeur init kernel
│   │   └── kpti_split.rs               # Tables scindées user/kernel (KPTI)
│   ├── vma/
│   │   ├── mod.rs
│   │   ├── descriptor.rs               # VMADesc: start, end, flags, backing
│   │   ├── tree.rs                     # Interval tree des VMAs (rbtree)
│   │   ├── operations.rs               # mmap/munmap/mprotect/mremap
│   │   └── cow.rs                      # CoW management
│   └── fault/
│       ├── mod.rs
│       ├── handler.rs                  # Page fault dispatcher
│       ├── cow.rs                      # CoW fault handler
│       ├── demand_paging.rs            # Demand paging
│       └── swap_in.rs                  # Swap-in handler
│
├── heap/                               # Allocateur dynamique kernel
│   ├── mod.rs
│   ├── allocator/
│   │   ├── mod.rs
│   │   ├── hybrid.rs                   # Dispatch slab/buddy par taille
│   │   ├── size_classes.rs             # 8B,16B,32B,...,2KB,4KB,large
│   │   └── global.rs                   # #[global_allocator] impl
│   ├── thread_local/
│   │   ├── mod.rs
│   │   ├── cache.rs                    # TLS cache (<25 cycles hot path)
│   │   ├── magazine.rs                 # Magazine layer (batch alloc/free)
│   │   └── drain.rs                    # Drain TLS vers pool global
│   └── large/
│       ├── mod.rs                      # ✅ AJOUT: mod.rs manquant dans l'original
│       └── vmalloc.rs                  # vmalloc (grandes allocs non-contiguës)
│
├── dma/                                # DMA Engine — sous memory/ (couche 0)
│   ├── mod.rs                          # ⚠️ dma/completion/wakeup appelle process/
│   │                                   # via DmaWakeupHandler trait (inversion dépendance)
│   │                                   # → zéro import direct de process/
│   ├── core/
│   │   ├── mod.rs                      # ✅ AJOUT: mod.rs manquant dans l'original
│   │   ├── types.rs                    # DmaAddr, DmaDesc, DmaChannel, DmaBuf
│   │   ├── descriptor.rs               # DmaRing 512 entrées, page-aligned
│   │   ├── mapping.rs                  # DMA coherent / streaming mappings
│   │   ├── wakeup_iface.rs             # DmaWakeupHandler trait (évite cycle vers process/)
│   │   └── error.rs                    # DmaError variants
│   ├── iommu/
│   │   ├── mod.rs                      # ✅ AJOUT: mod.rs manquant
│   │   ├── intel_vtd.rs                # Intel VT-d (DMAR)
│   │   ├── amd_iommu.rs                # AMD-Vi
│   │   ├── arm_smmu.rs                 # ARM SMMU (ARM future)
│   │   ├── domain.rs                   # IOMMU domain isolation par device
│   │   └── page_table.rs               # Page tables IOMMU 4 niveaux
│   ├── channels/
│   │   ├── mod.rs                      # ✅ AJOUT: mod.rs manquant
│   │   ├── manager.rs                  # Pool de canaux DMA
│   │   ├── channel.rs                  # Canal (ring producer/consumer)
│   │   ├── priority.rs                 # RT vs best-effort
│   │   └── affinity.rs                 # Canal ↔ CPU NUMA affinity
│   ├── engines/
│   │   ├── mod.rs                      # ✅ AJOUT: mod.rs manquant
│   │   ├── ioat.rs                     # Intel IOAT DMA Engine
│   │   ├── idxd.rs                     # Intel DSA
│   │   ├── ahci_dma.rs                 # ✅ AJOUT: AHCI/SATA (présent en v5, absent ici)
│   │   ├── nvme_dma.rs                 # NVMe PCIe natif
│   │   └── virtio_dma.rs               # VirtIO (VM)
│   ├── ops/
│   │   ├── mod.rs                      # ✅ AJOUT: mod.rs manquant
│   │   ├── memcpy.rs                   # DMA memcpy device↔RAM
│   │   ├── memset.rs                   # ✅ AJOUT: DMA memset (présent en v5, absent ici)
│   │   ├── scatter_gather.rs           # Scatter-Gather lists
│   │   ├── cyclic.rs                   # Cyclic DMA (audio/streaming)
│   │   └── interleaved.rs              # ✅ AJOUT: Interleaved RAID-like (présent en v5)
│   ├── completion/
│   │   ├── mod.rs                      # ✅ AJOUT: mod.rs manquant
│   │   ├── handler.rs                  # IRQ completion handler (<500ns)
│   │   ├── polling.rs                  # Polling haute fréquence
│   │   └── wakeup.rs                   # Wakeup via DmaWakeupHandler trait
│   └── stats/
│       ├── mod.rs                      # ✅ AJOUT: mod.rs manquant
│       └── counters.rs                 # Throughput, latency, errors
│
├── swap/
│   ├── mod.rs
│   ├── backend.rs                      # Backend (fichier ou partition)
│   ├── policy.rs                       # LRU/clock policy
│   ├── compress.rs                     # zswap (LZ4/ZSTD en RAM)
│   └── cluster.rs                      # Regroupement I/O swap
│
├── cow/
│   ├── mod.rs
│   ├── tracker.rs                      # Tracking pages partagées
│   └── breaker.rs                      # CoW break — copie physique
│
├── huge_pages/
│   ├── mod.rs
│   ├── thp.rs                          # THP 2MB
│   ├── hugetlbfs.rs                    # Huge pages dédiées 1GB
│   └── split.rs                        # Split 2MB → 512×4KB
│
├── protection/                         # Protection mémoire hardware
│   ├── mod.rs
│   ├── nx.rs                           # NX/XD bit (No-Execute)
│   ├── smep.rs                         # SMEP (Supervisor Mode Exec Prot)
│   ├── smap.rs                         # SMAP (Supervisor Mode Access Prot)
│   └── pku.rs                          # PKU — Protection Keys for Userspace (Intel)
│                                       # ✅ CORRIGÉ v4: "Intel MPX" supprimé (MPX ≠ PKU)
│
├── integrity/                          # Intégrité mémoire kernel
│   ├── mod.rs
│   ├── canary.rs                       # Stack canaries kernel
│   ├── guard_pages.rs                  # Pages garde (détection overflow)
│   └── sanitizer.rs                    # KASAN-like (debug builds)
│
├── numa/                               # NUMA support
│   ├── mod.rs
│   ├── node.rs                         # NUMANode descriptor
│   ├── distance.rs                     # Matrice distances
│   ├── policy.rs                       # Policies (local/interleaved/bind)
│   └── migration.rs                    # Migration pages inter-nœuds
│
└── utils/
    ├── mod.rs
    ├── futex_table.rs                  # FUTEX TABLE UNIQUE — adresse PHYSIQUE
    │                                   # scheduler/sync/ et ipc/sync/ délèguent ici
    ├── oom_killer.rs                   # OOM killer (thread dédié, emergency pool)
    └── shrinker.rs                     # MemoryShrinker trait (fs/ et ipc/ s'enregistrent)
```

---

## RÈGLES PAR SOUS-MODULE

---

### 📌 memory/physical/frame/emergency_pool.rs

**RÈGLE EMERGENCY-01 : Initialisation EN PREMIER absolu**

```rust
// kernel/src/memory/physical/frame/emergency_pool.rs

/// EmergencyPool — 64 WaitNode pré-alloués au BOOT, jamais libérés
/// DOIT être initialisé AVANT tout autre module.
/// UTILISÉ UNIQUEMENT par scheduler/sync/wait_queue.rs
///
/// ANTI-DEADLOCK CRITIQUE :
/// Sans ce pool, wait_queue.rs tenterait d'allouer via heap →
/// deadlock si l'allocateur heap est en train de reclaim mémoire.

pub const EMERGENCY_POOL_SIZE: usize = 64;

#[repr(C, align(64))]  // cache-line aligned
pub struct EmergencyPool {
    nodes: [MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE],
    /// Bitmap atomique : bit=1 → slot libre, bit=0 → utilisé
    free_bitmap: AtomicU64,
    initialized: AtomicBool,
}

static EMERGENCY_POOL: EmergencyPool = EmergencyPool {
    nodes: [MaybeUninit::uninit(); EMERGENCY_POOL_SIZE],
    free_bitmap: AtomicU64::new(u64::MAX),  // tous libres au boot
    initialized: AtomicBool::new(false),
};

pub fn init() {
    // Marquer comme initialisé — DOIT être la première chose au boot
    EMERGENCY_POOL.initialized.store(true, Ordering::Release);
}

/// Allouer un WaitNode depuis le pool d'urgence
/// ZÉRO appel à l'allocateur général — safe même en contexte de reclaim
pub fn alloc_wait_node() -> Option<&'static mut WaitNode> {
    debug_assert!(
        EMERGENCY_POOL.initialized.load(Ordering::Acquire),
        "EmergencyPool utilisé avant init() — bug d'ordre de démarrage"
    );

    loop {
        let bitmap = EMERGENCY_POOL.free_bitmap.load(Ordering::Acquire);
        if bitmap == 0 {
            return None;  // pool épuisé — augmenter EMERGENCY_POOL_SIZE
        }

        let slot = bitmap.trailing_zeros() as usize;
        let new_bitmap = bitmap & !(1u64 << slot);

        // CAS lock-free — réessayer si race
        match EMERGENCY_POOL.free_bitmap.compare_exchange_weak(
            bitmap, new_bitmap,
            Ordering::AcqRel, Ordering::Acquire,
        ) {
            Ok(_) => {
                // Slot obtenu — initialiser le WaitNode
                let node = unsafe {
                    EMERGENCY_POOL.nodes[slot].assume_init_mut()
                };
                node.reset();
                return Some(node);
            }
            Err(_) => continue,  // CAS raté → réessayer
        }
    }
}

/// Libérer un WaitNode vers le pool
pub fn free_wait_node(node: &'static mut WaitNode) {
    let slot = unsafe {
        let ptr = node as *mut WaitNode;
        let base = EMERGENCY_POOL.nodes.as_ptr() as usize;
        (ptr as usize - base) / size_of::<MaybeUninit<WaitNode>>()
    };

    debug_assert!(slot < EMERGENCY_POOL_SIZE);
    EMERGENCY_POOL.free_bitmap.fetch_or(1u64 << slot, Ordering::Release);
}
```

**RÈGLE EMERGENCY-02 : Surveillance du pool**

```rust
/// Vérifier le niveau du pool — à appeler périodiquement
pub fn check_pool_level() {
    let free = EMERGENCY_POOL.free_bitmap
        .load(Ordering::Relaxed)
        .count_ones();

    if free < 8 {
        // Moins de 8 slots libres → warning kernel (pas de panic)
        kernel_warn!("EmergencyPool critique: {} slots restants (min: 8)", free);
        // Solution: augmenter EMERGENCY_POOL_SIZE si ce warning apparaît
    }
}
```

---

### 📌 memory/utils/futex_table.rs

**RÈGLE FUTEX-01 : Table UNIQUE, indexée par adresse PHYSIQUE**

```rust
// kernel/src/memory/utils/futex_table.rs
//
// RÈGLE ABSOLUE : il n'existe QU'UNE SEULE table futex dans tout l'OS.
// scheduler/sync/ et ipc/sync/ DÉLÈGUENT à cette table.
// NE PAS dupliquer cette logique ailleurs.
//
// Indexation par adresse PHYSIQUE (pas virtuelle) car :
// - Deux processus peuvent mapper la même page physique à des adresses virtuelles différentes
// - Index par virtuel → deux processus sur le même futex physique ne se voient pas
// - Index par physique → comportement correct garanti

pub const FUTEX_HASH_BUCKETS: usize = 256;

pub struct FutexTable {
    buckets: [SpinLock<FutexBucket>; FUTEX_HASH_BUCKETS],
}

impl FutexTable {
    pub fn wait(
        &self,
        phys_addr: PhysAddr,
        expected_value: u32,
        timeout: Option<Duration>,
    ) -> FutexResult {
        let bucket_idx = self.hash(phys_addr);
        let mut bucket = self.buckets[bucket_idx].lock();

        let current = unsafe { *(phys_addr.as_ptr::<u32>()) };
        if current != expected_value {
            return FutexResult::WouldBlock;
        }

        // Allouer waiter depuis EmergencyPool (JAMAIS depuis heap)
        // ✅ CORRIGÉ: chemin d'accès complet et cohérent avec l'arborescence
        let waiter = memory::physical::frame::emergency_pool::alloc_wait_node()
            .expect("EmergencyPool épuisé");
        waiter.phys_addr = phys_addr;
        waiter.thread_id = current_thread_id();

        bucket.insert(waiter);
        drop(bucket);  // LIBÉRER le lock AVANT de se bloquer

        THREAD_BLOCKER.block_current(timeout)
    }

    pub fn wake(&self, phys_addr: PhysAddr, count: u32) -> u32 {
        let bucket_idx = self.hash(phys_addr);
        let mut bucket = self.buckets[bucket_idx].lock();
        let mut woken = 0u32;

        while woken < count {
            match bucket.pop_waiter(phys_addr) {
                Some(waiter) => {
                    THREAD_WAKER.wake(waiter.thread_id);
                    // ✅ CORRIGÉ: chemin cohérent
                    memory::physical::frame::emergency_pool::free_wait_node(waiter);
                    woken += 1;
                }
                None => break,
            }
        }
        woken
    }

    fn hash(&self, phys_addr: PhysAddr) -> usize {
        let addr = phys_addr.as_u64();
        ((addr >> 2) ^ (addr >> 12)) as usize & (FUTEX_HASH_BUCKETS - 1)
    }
}

/// Interfaces abstraites — memory/ ne peut pas importer scheduler/
/// Enregistrées par scheduler/ au boot
static THREAD_BLOCKER: spin::Once<&'static dyn ThreadBlocker> = spin::Once::new();
static THREAD_WAKER:   spin::Once<&'static dyn ThreadWaker>   = spin::Once::new();

pub trait ThreadBlocker: Send + Sync {
    fn block_current(&self, timeout: Option<Duration>) -> FutexResult;
}
pub trait ThreadWaker: Send + Sync {
    fn wake(&self, thread_id: ThreadId);
}

pub fn register_thread_blocker(b: &'static dyn ThreadBlocker) {
    THREAD_BLOCKER.call_once(|| b);
}
pub fn register_thread_waker(w: &'static dyn ThreadWaker) {
    THREAD_WAKER.call_once(|| w);
}
```

> ⚠️ **ERREUR CORRIGÉE** dans le code original :
> ```rust
> // AVANT (incorrect) :
> let waiter = memory::frame::emergency_pool::alloc_wait_node()
> // APRÈS (correct) :
> let waiter = memory::physical::frame::emergency_pool::alloc_wait_node()
> ```
> Le chemin `memory::frame::emergency_pool` n'existe pas dans l'arborescence.
> Le module `emergency_pool` est dans `memory/physical/frame/emergency_pool.rs`.

---

### 📌 memory/dma/core/wakeup_iface.rs

**RÈGLE DMA-WAKEUP : Briser le cycle memory → process via trait abstrait**

```rust
// kernel/src/memory/dma/core/wakeup_iface.rs

pub trait DmaWakeupHandler: Send + Sync {
    fn wakeup_thread(&self, thread_id: ThreadId, result: Result<(), DmaError>);
}

static DMA_WAKEUP: spin::Once<&'static dyn DmaWakeupHandler> = spin::Once::new();

pub fn register_wakeup_handler(handler: &'static dyn DmaWakeupHandler) {
    DMA_WAKEUP.call_once(|| handler);
}

pub fn wakeup_thread(thread_id: ThreadId, result: Result<(), DmaError>) {
    DMA_WAKEUP
        .get()
        .expect("DmaWakeupHandler non enregistré — vérifier ordre init boot")
        .wakeup_thread(thread_id, result);
}
```

---

### 📌 memory/virtual/address_space/tlb.rs

**RÈGLE TLB-01 : Shootdown synchrone avant libération**

```rust
// RÈGLE ABSOLUE : unmap → flush local → IPI synchrone ACK → free
// JAMAIS libérer le frame avant que tous les CPUs aient flushé

pub fn tlb_shootdown(addr_space: &AddressSpace, start: VirtAddr, end: VirtAddr) {
    unsafe { flush_tlb_range_local(start, end); }

    let target_cpus = addr_space.active_cpus.load(Ordering::Acquire);
    if target_cpus == 0 || target_cpus == current_cpu_mask() {
        return;
    }

    let shootdown_req = TlbShootdownRequest {
        start,
        end,
        ack_counter: AtomicU32::new(target_cpus.count_ones()),
    };

    arch::apic::send_ipi_to_mask(
        target_cpus,
        IpiVector::TLB_SHOOTDOWN,
        &shootdown_req as *const _ as u64,
    );

    // Spin — doit être court <10µs
    while shootdown_req.ack_counter.load(Ordering::Acquire) > 0 {
        core::hint::spin_loop();
    }
    // MAINTENANT safe de libérer le frame physique
}

pub fn handle_tlb_shootdown_ipi(req: *const TlbShootdownRequest) {
    let req = unsafe { &*req };
    unsafe { flush_tlb_range_local(req.start, req.end); }
    req.ack_counter.fetch_sub(1, Ordering::Release);
}
```

---

### 📌 memory/physical/allocator/buddy.rs

**RÈGLE BUDDY-01 : Hot path sans lock si per-CPU pool disponible**

```rust
pub const MAX_ORDER: usize = 11;  // Orders 0 (4KB) à 10 (4MB)

pub fn alloc_pages(order: u32, flags: AllocFlags) -> Result<PhysFrame, AllocError> {
    // Fast path: per-CPU pool (order 0 uniquement, lock-free)
    if order == 0 {
        if let Some(frame) = percpu_pool::pop() {
            return Ok(frame);
        }
    }

    // Hint NUMA optionnel (ai_hints.rs — lecture seule)
    let preferred_zone = ai_hints::hint_numa_node(order as u8, current_cpu())
        .map(|n| zone_for_numa(n, flags))
        .unwrap_or_else(|| zone_for_flags(flags));

    buddy_alloc_internal(order, preferred_zone, flags)
}

pub fn free_pages(frame: PhysFrame, order: u32) {
    if order == 0 {
        if percpu_pool::push(frame) {
            return;
        }
    }
    // Coalescer avec le buddy (fusion avec bloc adjacent si libre)
    let zone = zone_of(frame.phys_addr());
    coalesce_and_free(frame, order, zone);
}
```

---

### 📌 memory/physical/frame/pool.rs

**RÈGLE POOL-01 : Per-CPU pools lock-free, séparé de EmergencyPool**

```rust
pub const PERCPU_POOL_SIZE: usize = 512;
pub const DRAIN_THRESHOLD: usize = 400;

#[repr(C, align(64))]
pub struct PerCpuFramePool {
    frames: [MaybeUninit<PhysFrame>; PERCPU_POOL_SIZE],
    count: usize,
    _pad: [u8; 64 - size_of::<usize>() % 64],
}

#[inline(always)]
pub fn pop() -> Option<PhysFrame> {
    let pool = percpu::get_mut::<PerCpuFramePool>();
    if pool.count == 0 { return None; }
    pool.count -= 1;
    Some(unsafe { pool.frames[pool.count].assume_init() })
}

#[inline(always)]
pub fn push(frame: PhysFrame) -> bool {
    let pool = percpu::get_mut::<PerCpuFramePool>();
    if pool.count >= PERCPU_POOL_SIZE {
        drain_to_buddy(pool, PERCPU_POOL_SIZE / 2);
    }
    pool.frames[pool.count] = MaybeUninit::new(frame);
    pool.count += 1;
    true
}

fn drain_to_buddy(pool: &mut PerCpuFramePool, count: usize) {
    for _ in 0..count {
        if pool.count == 0 { break; }
        pool.count -= 1;
        let frame = unsafe { pool.frames[pool.count].assume_init() };
        buddy::free_pages(frame, 0);
    }
}
```

---

### 📌 memory/virtual/vma/cow.rs

**RÈGLE COW-01 : Fork CoW — TLB flush parent AVANT retour**

```rust
pub fn fork_address_space(parent: &AddressSpace) -> Result<AddressSpace, MemError> {
    let child = AddressSpace::new_empty()?;

    for vma in parent.vmas.iter() {
        let child_vma = vma.clone_for_fork();

        if vma.flags.contains(VmaFlags::WRITE) {
            parent.page_table.protect_range(
                vma.start, vma.end,
                PageFlags::PRESENT | PageFlags::USER,  // sans WRITE
            )?;

            for page in vma.pages() {
                if let Some(frame) = page.physical_frame() {
                    frame.ref_count.fetch_add(1, Ordering::Relaxed);
                    frame.flags.insert(FrameFlags::COW);
                }
            }
        }
        child.vmas.insert(child_vma);
    }

    // TLB flush du parent OBLIGATOIRE — sinon le parent peut encore écrire
    // sur des pages maintenant partagées (PROC-08)
    parent.flush_tlb_all();  // IPI si SMP

    Ok(child)
}
```

---

### 📌 memory/utils/oom_killer.rs

**RÈGLE OOM-01 : Thread dédié, jamais appelé depuis le hot path**

```rust
// Non-bloquant — notifie le thread dédié et retourne immédiatement
pub fn trigger_oom() {
    *OOM_KILLER.notify.lock() = true;
    OOM_KILLER.notify_condvar.notify_one();
}

// Critères stricts : ne pas tuer kthreads, RT critiques, DMA actif, PID 1
fn select_oom_victim() -> Option<Pid> {
    OOM_VICTIM_SELECTOR.get()?.select_victim(&OomCriteria {
        exclude_kthreads:    true,
        exclude_rt_critical: true,
        exclude_dma_active:  true,
        exclude_pid1:        true,
    })
}
```

---

## ORDRE D'INITIALISATION MEMORY/

```
SÉQUENCE OBLIGATOIRE (panique si ordre incorrect) :

1. memory::physical::frame::emergency_pool::init()    ← EN PREMIER ABSOLU
2. memory::core::layout::init(e820_map)               ← Carte mémoire
3. memory::physical::allocator::bitmap::bootstrap()   ← Allocateur minimal
4. memory::physical::allocator::buddy::init()         ← Buddy complet
5. memory::heap::allocator::global::init()            ← Heap kernel (#[global_allocator])
6. memory::utils::futex_table::init()                 ← Table futex
7. memory::physical::frame::pool::init_percpu()       ← Per-CPU pools (après SMP)
8. memory::dma::iommu::init()                         ← IOMMU (après PCI enum)
9. memory::utils::oom_killer::start_thread()          ← OOM killer thread
```

**Code d'assertion d'ordre :**

```rust
// kernel/src/memory/mod.rs

static INIT_PHASE: AtomicU8 = AtomicU8::new(0);

macro_rules! assert_phase {
    ($expected:expr, $name:expr) => {
        let phase = INIT_PHASE.load(Ordering::Acquire);
        assert_eq!(phase, $expected,
            "Ordre d'init mémoire incorrect: {} attendu phase {}, phase actuelle {}",
            $name, $expected, phase);
        INIT_PHASE.store($expected + 1, Ordering::Release);
    }
}
```

---

## TABLEAU DES RÈGLES MEMORY/ (référence rapide)

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — memory/ (couche 0)                           │
├────────────────────────────────────────────────────────────────┤
│ MEM-01  │ Aucun import de scheduler/, ipc/, fs/, process/      │
│ MEM-02  │ EmergencyPool initialisé EN PREMIER (avant buddy)    │
│ MEM-03  │ FutexTable = adresse PHYSIQUE, table UNIQUE          │
│ MEM-04  │ TLB shootdown synchrone AVANT free_pages()           │
│ MEM-05  │ DMA frames : FrameFlags::DMA_PINNED jusqu'au ACK     │
│ MEM-06  │ Pages IPC : FrameFlags::NO_COW obligatoire           │
│ MEM-07  │ Huge pages : interdit split si DMA_PINNED            │
│ MEM-08  │ OOM killer = thread dédié, JAMAIS appelé depuis hot  │
│          │ path reclaim                                         │
│ MEM-09  │ WakeupHandler DMA = trait abstrait (pas d'import     │
│          │ process/)                                            │
│ MEM-10  │ Per-CPU pool (pool.rs) SÉPARÉ de EmergencyPool       │
│          │ (emergency_pool.rs)                                  │
│ MEM-11  │ CoW fork : flush TLB parent AVANT retour fork()      │
│ MEM-12  │ Allocations en contexte IRQ : AllocFlags::ATOMIC     │
│          │ uniquement (depuis per-CPU pool, jamais buddy)       │
│ MEM-13  │ reclaim.rs : flag PF_MEMALLOC sur thread reclaimeur  │
│          │ pour éviter deadlock récursif                        │
│ MEM-14  │ swap/backend.rs : vérifier is_in_reclaim_context()   │
│          │ avant tout sleep                                      │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS ABSOLUS                                              │
├────────────────────────────────────────────────────────────────┤
│ ✗  use crate::scheduler dans memory/                           │
│ ✗  use crate::process dans memory/  (sauf trait abstrait)      │
│ ✗  Allouer depuis heap dans #[no_mangle] exception handlers    │
│ ✗  Libérer un frame DMA_PINNED sans wait_dma_complete()        │
│ ✗  Splitter une huge page DMA_PINNED                           │
│ ✗  Double table futex (une seule dans memory/utils/)           │
│ ✗  EmergencyPool utilisé pour autre chose que WaitNodes        │
│ ✗  memory::frame::emergency_pool (chemin incorrect)            │
│    → utiliser memory::physical::frame::emergency_pool          │
└────────────────────────────────────────────────────────────────┘
```

---

## 📋 CORRECTIONS APPORTÉES À DOC2

| # | Localisation | Erreur | Correction |
|---|---|---|---|
| 1 | Header position | `EST APPELÉ PAR: [...] dma/` | Retiré — `dma/` est **dans** `memory/`, pas appelant |
| 2 | `heap/large/` | `mod.rs` manquant | Ajouté |
| 3 | `dma/core/` | `mod.rs` manquant | Ajouté |
| 4 | `dma/iommu/` | `mod.rs` manquant + `ahci_dma.rs` absent | Ajoutés |
| 5 | `dma/channels/` | `mod.rs` manquant | Ajouté |
| 6 | `dma/engines/` | `mod.rs` + `ahci_dma.rs` manquants | Ajoutés |
| 7 | `dma/ops/` | `mod.rs` + `memset.rs` + `interleaved.rs` manquants | Ajoutés |
| 8 | `dma/completion/` | `mod.rs` manquant | Ajouté |
| 9 | `dma/stats/` | `mod.rs` manquant | Ajouté |
| 10 | `futex_table.rs` code | `memory::frame::emergency_pool` | Corrigé → `memory::physical::frame::emergency_pool` |
| 11 | `futex_table.rs` code | `static THREAD_BLOCKER/WAKER: &dyn ...` initialisé avec `&DefaultBlocker` fictif | Remplacé par `spin::Once` correctement initialisable |
| 12 | `protection/pku.rs` | Commentaire absent | Ajouté — PKU ≠ MPX (correction v4 conservée) |

---

*DOC 2 — Module Memory — Exo-OS — v5 corrigé*
*Prochains : DOC 3 (Scheduler) · DOC 4 (Process/Signal) · DOC 5 (IPC) · DOC 6 (FS) · DOC 7 (Security/Capability) · DOC 8 (DMA) · DOC 9 (Shield)*
