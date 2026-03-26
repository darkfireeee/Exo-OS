# Audit approfondi du module `memory` (Exo-OS)

Date: 2026-03-22
Périmètre: `kernel/src/memory/**`
Objectif: base de refonte/correction de la couche 0

---

## 1) Résumé exécutif

Le module `memory` est la couche 0 du noyau.
Il définit les primitives de base pour tout le reste.
Il gère mémoire physique, virtuelle, heap, DMA, swap, COW, huge pages, protections.
Il contient aussi des utilitaires critiques (`futex_table`, OOM killer, shrinkers).

Points forts:
- architecture modulaire claire
- API publique bien ré-exportée dans `memory/mod.rs`
- séparation forte des sous-domaines
- forte présence d’atomiques et structures no_std compatibles

Points de risque:
- surface de code très large (128 fichiers)
- beaucoup de chemins concurrency-sensitive
- interfaces avec `arch` et `scheduler` très délicates
- stubs ciblés (NUMA allocator, arm_smmu)

---

## 2) Rôle et invariants de couche

`memory` doit rester autonome.
`memory` ne doit pas dépendre de `scheduler/process/ipc/fs/security`.
`memory` sert de fondation aux autres couches.
`memory` gère les allocations critiques de boot.
`memory` porte la gestion d’adressage kernel/user.
`memory` pilote TLB shootdowns côté VM.
`memory` doit préserver lock ordering global.
`memory` doit maintenir des chemins no-alloc en contexte sensible.
`memory` doit être robuste en OOM.

Invariants clés:
- Emergency pool initialisé très tôt.
- `futex_table` singleton unique.
- cohérence CR3/page tables via interfaces arch.
- shootdown TLB avant libération frames.
- cohérence refcount COW.
- mappings SHM/VM stables et non ambigus.

---

## 3) Cartographie exhaustive des fichiers

### 3.1 Racine
- `kernel/src/memory/mod.rs`
- `kernel/src/memory/arch_iface.rs`
- `kernel/src/memory/numa.rs`

### 3.2 `core`
- `kernel/src/memory/core/mod.rs`
- `kernel/src/memory/core/address.rs`
- `kernel/src/memory/core/constants.rs`
- `kernel/src/memory/core/layout.rs`
- `kernel/src/memory/core/types.rs`

### 3.3 `cow`
- `kernel/src/memory/cow/mod.rs`
- `kernel/src/memory/cow/breaker.rs`
- `kernel/src/memory/cow/tracker.rs`

### 3.4 `dma/channels`
- `kernel/src/memory/dma/channels/mod.rs`
- `kernel/src/memory/dma/channels/affinity.rs`
- `kernel/src/memory/dma/channels/channel.rs`
- `kernel/src/memory/dma/channels/manager.rs`
- `kernel/src/memory/dma/channels/priority.rs`

### 3.5 `dma/completion`
- `kernel/src/memory/dma/completion/mod.rs`
- `kernel/src/memory/dma/completion/handler.rs`
- `kernel/src/memory/dma/completion/polling.rs`
- `kernel/src/memory/dma/completion/wakeup.rs`

### 3.6 `dma/core`
- `kernel/src/memory/dma/core/mod.rs`
- `kernel/src/memory/dma/core/descriptor.rs`
- `kernel/src/memory/dma/core/error.rs`
- `kernel/src/memory/dma/core/mapping.rs`
- `kernel/src/memory/dma/core/types.rs`
- `kernel/src/memory/dma/core/wakeup_iface.rs`

### 3.7 `dma/engines`
- `kernel/src/memory/dma/engines/mod.rs`
- `kernel/src/memory/dma/engines/ahci_dma.rs`
- `kernel/src/memory/dma/engines/idxd.rs`
- `kernel/src/memory/dma/engines/ioat.rs`
- `kernel/src/memory/dma/engines/nvme_dma.rs`
- `kernel/src/memory/dma/engines/virtio_dma.rs`

### 3.8 `dma/iommu`
- `kernel/src/memory/dma/iommu/mod.rs`
- `kernel/src/memory/dma/iommu/amd_iommu.rs`
- `kernel/src/memory/dma/iommu/arm_smmu.rs`
- `kernel/src/memory/dma/iommu/domain.rs`
- `kernel/src/memory/dma/iommu/intel_vtd.rs`
- `kernel/src/memory/dma/iommu/page_table.rs`

### 3.9 `dma/ops`
- `kernel/src/memory/dma/ops/mod.rs`
- `kernel/src/memory/dma/ops/cyclic.rs`
- `kernel/src/memory/dma/ops/interleaved.rs`
- `kernel/src/memory/dma/ops/memcpy.rs`
- `kernel/src/memory/dma/ops/memset.rs`
- `kernel/src/memory/dma/ops/scatter_gather.rs`

### 3.10 `dma/stats`
- `kernel/src/memory/dma/stats/mod.rs`
- `kernel/src/memory/dma/stats/counters.rs`

### 3.11 `dma` racine
- `kernel/src/memory/dma/mod.rs`

### 3.12 `heap/allocator`
- `kernel/src/memory/heap/allocator/mod.rs`
- `kernel/src/memory/heap/allocator/global.rs`
- `kernel/src/memory/heap/allocator/hybrid.rs`
- `kernel/src/memory/heap/allocator/size_classes.rs`

### 3.13 `heap/large`
- `kernel/src/memory/heap/large/mod.rs`
- `kernel/src/memory/heap/large/vmalloc.rs`

### 3.14 `heap/thread_local`
- `kernel/src/memory/heap/thread_local/mod.rs`
- `kernel/src/memory/heap/thread_local/cache.rs`
- `kernel/src/memory/heap/thread_local/drain.rs`
- `kernel/src/memory/heap/thread_local/magazine.rs`

### 3.15 `heap` racine
- `kernel/src/memory/heap/mod.rs`

### 3.16 `huge_pages`
- `kernel/src/memory/huge_pages/mod.rs`
- `kernel/src/memory/huge_pages/hugetlbfs.rs`
- `kernel/src/memory/huge_pages/split.rs`
- `kernel/src/memory/huge_pages/thp.rs`

### 3.17 `integrity`
- `kernel/src/memory/integrity/mod.rs`
- `kernel/src/memory/integrity/canary.rs`
- `kernel/src/memory/integrity/guard_pages.rs`
- `kernel/src/memory/integrity/sanitizer.rs`

### 3.18 `physical/allocator`
- `kernel/src/memory/physical/allocator/mod.rs`
- `kernel/src/memory/physical/allocator/bitmap.rs`
- `kernel/src/memory/physical/allocator/buddy.rs`
- `kernel/src/memory/physical/allocator/numa_aware.rs`
- `kernel/src/memory/physical/allocator/numa_hints.rs`
- `kernel/src/memory/physical/allocator/slab.rs`
- `kernel/src/memory/physical/allocator/slub.rs`

### 3.19 `physical/frame`
- `kernel/src/memory/physical/frame/mod.rs`
- `kernel/src/memory/physical/frame/descriptor.rs`
- `kernel/src/memory/physical/frame/emergency_pool.rs`
- `kernel/src/memory/physical/frame/pool.rs`
- `kernel/src/memory/physical/frame/reclaim.rs`
- `kernel/src/memory/physical/frame/ref_count.rs`

### 3.20 `physical/numa`
- `kernel/src/memory/physical/numa/mod.rs`
- `kernel/src/memory/physical/numa/distance.rs`
- `kernel/src/memory/physical/numa/migration.rs`
- `kernel/src/memory/physical/numa/node.rs`
- `kernel/src/memory/physical/numa/policy.rs`

### 3.21 `physical/zone`
- `kernel/src/memory/physical/zone/mod.rs`
- `kernel/src/memory/physical/zone/dma.rs`
- `kernel/src/memory/physical/zone/dma32.rs`
- `kernel/src/memory/physical/zone/high.rs`
- `kernel/src/memory/physical/zone/movable.rs`
- `kernel/src/memory/physical/zone/normal.rs`

### 3.22 `physical` racine
- `kernel/src/memory/physical/mod.rs`
- `kernel/src/memory/physical/stats.rs`

### 3.23 `protection`
- `kernel/src/memory/protection/mod.rs`
- `kernel/src/memory/protection/nx.rs`
- `kernel/src/memory/protection/pku.rs`
- `kernel/src/memory/protection/smap.rs`
- `kernel/src/memory/protection/smep.rs`

### 3.24 `swap`
- `kernel/src/memory/swap/mod.rs`
- `kernel/src/memory/swap/backend.rs`
- `kernel/src/memory/swap/cluster.rs`
- `kernel/src/memory/swap/compress.rs`
- `kernel/src/memory/swap/policy.rs`

### 3.25 `utils`
- `kernel/src/memory/utils/mod.rs`
- `kernel/src/memory/utils/futex_table.rs`
- `kernel/src/memory/utils/oom_killer.rs`
- `kernel/src/memory/utils/shrinker.rs`

### 3.26 `virtual/address_space`
- `kernel/src/memory/virtual/address_space/mod.rs`
- `kernel/src/memory/virtual/address_space/kernel.rs`
- `kernel/src/memory/virtual/address_space/mapper.rs`
- `kernel/src/memory/virtual/address_space/tlb.rs`
- `kernel/src/memory/virtual/address_space/user.rs`

### 3.27 `virtual/fault`
- `kernel/src/memory/virtual/fault/mod.rs`
- `kernel/src/memory/virtual/fault/cow.rs`
- `kernel/src/memory/virtual/fault/demand_paging.rs`
- `kernel/src/memory/virtual/fault/handler.rs`
- `kernel/src/memory/virtual/fault/swap_in.rs`

### 3.28 `virtual/page_table`
- `kernel/src/memory/virtual/page_table/mod.rs`
- `kernel/src/memory/virtual/page_table/builder.rs`
- `kernel/src/memory/virtual/page_table/kpti_split.rs`
- `kernel/src/memory/virtual/page_table/walker.rs`
- `kernel/src/memory/virtual/page_table/x86_64.rs`

### 3.29 `virtual/vma`
- `kernel/src/memory/virtual/vma/mod.rs`
- `kernel/src/memory/virtual/vma/cow.rs`
- `kernel/src/memory/virtual/vma/descriptor.rs`
- `kernel/src/memory/virtual/vma/operations.rs`
- `kernel/src/memory/virtual/vma/tree.rs`

### 3.30 `virtual` racine
- `kernel/src/memory/virtual/mod.rs`
- `kernel/src/memory/virtual/mmap.rs`

---

## 4) APIs publiques structurantes

APIs transverses exposées par `memory/mod.rs`:
- `alloc_page`, `alloc_pages`, `free_page`, `free_pages`
- `heap_alloc`, `heap_free`
- `drain_on_context_switch`, `drain_on_memory_pressure`
- `DmaWakeupHandler`
- `SwapDevice`, `SwapSlot`, `should_swap`, `is_critical`
- `COW_TRACKER`
- `alloc_huge_page`, `split_huge_page`, `try_promote_to_huge`
- `copy_from_user`, `copy_to_user`, `zero_user`, `nx_page_flags`
- `kasan_on_alloc`, `kasan_on_free`, `kasan_check_access`
- `FUTEX_TABLE`, `futex_wait`, `futex_wake`, `futex_requeue`
- `register_oom_kill_sender`, `oom_kill`
- `register_shrinker`, `run_shrinkers`

---

## 5) Concurrence, verrous, atomiques

Patterns majeurs observés:
- `spin::Mutex` (ex: `futex_table`, `oom_killer`, `shrinker`, `pku`).
- `spin::RwLock` (ex: THP mode, policy NUMA/swap watermarks).
- atomiques massifs (`AtomicU64`, `AtomicU32`, `AtomicPtr`, `AtomicBool`).
- locks scheduler spécifiques via imports ciblés sur quelques modules.

Points d’attention:
- contention potentielle sur `futex_table` buckets chauds.
- cohérence des orderings (Acquire/Release vs Relaxed).
- chemins OOM doivent rester non bloquants.

---

## 6) `&str`, signatures texte, et empreinte API

Présence de `&str` notable dans:
- messages d’erreur/diagnostic.
- interfaces quota/tracing (via FS et utilitaires).
- fonctions utilitaires de reportings.

Risque faible mais:
- éviter allocations implicites en chemins chauds.
- éviter formatting coûteux dans handlers critiques.

---

## 7) TODO/stub/placeholder relevés

- `physical/frame/pool.rs` : TODO support >64 CPUs bitmap.
- `physical/allocator/numa_aware.rs` : stub interleave simplifié.
- `dma/iommu/arm_smmu.rs` : NotSupported + TODO stage-1 walk.
- `virtual/page_table/kpti_split.rs` : commentaires de stub inter-CPU.

Impact refonte:
- scalabilité SMP limitée si >64.
- portabilité ARM incomplète.
- NUMA policy partiellement simplifiée.

---

## 8) Journal de contrôle détaillé (MEM-CHK)

- MEM-CHK-001 vérifier règle couche0 sans dépendance montante.
- MEM-CHK-002 vérifier EmergencyPool initialisé en premier.
- MEM-CHK-003 vérifier cohérence constants PAGE_SIZE/HUGE_PAGE.
- MEM-CHK-004 vérifier alignements PhysAddr/VirtAddr.
- MEM-CHK-005 vérifier conversions physmap sans overflow.
- MEM-CHK-006 vérifier zones DMA/DMA32 bornes correctes.
- MEM-CHK-007 vérifier ordre init phases 1..8.
- MEM-CHK-008 vérifier init bitmap bootstrap bornes physiques.
- MEM-CHK-009 vérifier free_region ignore régions invalides.
- MEM-CHK-010 vérifier buddy split/merge correctness.
- MEM-CHK-011 vérifier buddy order max cohérent.
- MEM-CHK-012 vérifier free list corruption guards.
- MEM-CHK-013 vérifier slab classes cohérentes tailles.
- MEM-CHK-014 vérifier slub freelist encoding robuste.
- MEM-CHK-015 vérifier fallback alloc si slub échoue.
- MEM-CHK-016 vérifier vmalloc fallback large allocations.
- MEM-CHK-017 vérifier heap allocator GlobalAlloc sécurité.
- MEM-CHK-018 vérifier double-free protections.
- MEM-CHK-019 vérifier lifetime magazine TLS.
- MEM-CHK-020 vérifier drain TLS en context switch.
- MEM-CHK-021 vérifier drain pressure path.
- MEM-CHK-022 vérifier huge page promote conditions.
- MEM-CHK-023 vérifier huge page split conditions.
- MEM-CHK-024 vérifier hugetlb pool accounting.
- MEM-CHK-025 vérifier canary init entropy suffisante.
- MEM-CHK-026 vérifier canary verify sur chemins critiques.
- MEM-CHK-027 vérifier guard pages mapping NX.
- MEM-CHK-028 vérifier sanitizer shadow access borné.
- MEM-CHK-029 vérifier PKU key allocator thread-safe.
- MEM-CHK-030 vérifier SMEP enable conditionnel.
- MEM-CHK-031 vérifier SMAP guard usage correct.
- MEM-CHK-032 vérifier NX flags appliqués pages data.
- MEM-CHK-033 vérifier copy_to/from_user validations.
- MEM-CHK-034 vérifier page table walker bounds.
- MEM-CHK-035 vérifier walker allocate flags cohérents.
- MEM-CHK-036 vérifier `read_cr3/write_cr3` wrappers.
- MEM-CHK-037 vérifier invlpg usage ciblé.
- MEM-CHK-038 vérifier TLB flush single/range/all.
- MEM-CHK-039 vérifier shootdown queue overflow handling.
- MEM-CHK-040 vérifier remote ack timeout.
- MEM-CHK-041 vérifier free_pages post-shootdown strict.
- MEM-CHK-042 vérifier VMA insert ordering.
- MEM-CHK-043 vérifier VMA overlap reject.
- MEM-CHK-044 vérifier VMA split correctness.
- MEM-CHK-045 vérifier mmap gap finder robustesse.
- MEM-CHK-046 vérifier mprotect rights transitions.
- MEM-CHK-047 vérifier munmap partiel/total.
- MEM-CHK-048 vérifier do_brk bornes userspace.
- MEM-CHK-049 vérifier current_as getter registration.
- MEM-CHK-050 vérifier page fault context construction.
- MEM-CHK-051 vérifier demand paging anon path.
- MEM-CHK-052 vérifier demand paging file path.
- MEM-CHK-053 vérifier COW break atomicity.
- MEM-CHK-054 vérifier COW refcount decrement path.
- MEM-CHK-055 vérifier swap_in provider null path.
- MEM-CHK-056 vérifier swap_in timeout/error path.
- MEM-CHK-057 vérifier fault stats increments cohérents.
- MEM-CHK-058 vérifier `KERNEL_AS` singleton init once.
- MEM-CHK-059 vérifier user AS clone/fork semantics.
- MEM-CHK-060 vérifier mapper route kernel/user correct.
- MEM-CHK-061 vérifier DMA domain table init.
- MEM-CHK-062 vérifier descriptor table init.
- MEM-CHK-063 vérifier wakeup handler registration.
- MEM-CHK-064 vérifier wake_on_completion error path.
- MEM-CHK-065 vérifier channel manager saturation.
- MEM-CHK-066 vérifier DMA priority arbitration.
- MEM-CHK-067 vérifier affinity policy application.
- MEM-CHK-068 vérifier scatter_gather bounds.
- MEM-CHK-069 vérifier memset/memcpy engine fallback.
- MEM-CHK-070 vérifier NVMe DMA path alignment.
- MEM-CHK-071 vérifier AHCI DMA error handling.
- MEM-CHK-072 vérifier IOAT init gating.
- MEM-CHK-073 vérifier IDXD init gating.
- MEM-CHK-074 vérifier VirtIO DMA compatibility.
- MEM-CHK-075 vérifier VT-d domain mapping validity.
- MEM-CHK-076 vérifier AMD IOMMU config bits.
- MEM-CHK-077 vérifier ARM SMMU returns NotSupported proprement.
- MEM-CHK-078 vérifier IOVA allocator wrap-around.
- MEM-CHK-079 vérifier DMA stats atomics ordering.
- MEM-CHK-080 vérifier swap backend trait contracts.
- MEM-CHK-081 vérifier slot alloc/free consistency.
- MEM-CHK-082 vérifier CLOCK policy fairness.
- MEM-CHK-083 vérifier compress store/load symmetry.
- MEM-CHK-084 vérifier cluster manager locking.
- MEM-CHK-085 vérifier watermarks swap rwlock usage.
- MEM-CHK-086 vérifier `should_swap` thresholds.
- MEM-CHK-087 vérifier `is_critical` true conditions.
- MEM-CHK-088 vérifier OOM scorer fallback.
- MEM-CHK-089 vérifier OOM sender registered before use.
- MEM-CHK-090 vérifier OOM suppress/unsuppress symmetry.
- MEM-CHK-091 vérifier shrinker register/unregister safety.
- MEM-CHK-092 vérifier shrinkers priorité order.
- MEM-CHK-093 vérifier shrinkers no panic policy.
- MEM-CHK-094 vérifier futex seed init once.
- MEM-CHK-095 vérifier futex hash key randomness.
- MEM-CHK-096 vérifier futex bucket lock scope réduit.
- MEM-CHK-097 vérifier futex wait cancellation path.
- MEM-CHK-098 vérifier futex requeue lock ordering.
- MEM-CHK-099 vérifier futex wake_n semantics.
- MEM-CHK-100 vérifier futex timeout path.
- MEM-CHK-101 vérifier ref_count underflow guard.
- MEM-CHK-102 vérifier frame descriptor table bounds.
- MEM-CHK-103 vérifier MAX_PHYS_FRAMES consistency.
- MEM-CHK-104 vérifier per-CPU pool init each CPU.
- MEM-CHK-105 vérifier per-CPU pool drain thresholds.
- MEM-CHK-106 vérifier reclaim path non-réentrant.
- MEM-CHK-107 vérifier reclaim + swap interaction.
- MEM-CHK-108 vérifier zone selection by flags.
- MEM-CHK-109 vérifier addr_satisfies_flags correctness.
- MEM-CHK-110 vérifier NUMA node distance matrix.
- MEM-CHK-111 vérifier NUMA policy default stable.
- MEM-CHK-112 vérifier migration cross-node safety.
- MEM-CHK-113 vérifier migration page lock discipline.
- MEM-CHK-114 vérifier numa hints bounded outputs.
- MEM-CHK-115 vérifier numa_aware fallback global.
- MEM-CHK-116 vérifier `arch_iface` IPI vectors align arch.
- MEM-CHK-117 vérifier `register_tlb_ipi_sender` called early.
- MEM-CHK-118 vérifier no deadlock IPC<Sched<Mem<FS.
- MEM-CHK-119 vérifier scheduler lock non pris en mémoire critique.
- MEM-CHK-120 vérifier filesystem callbacks via shrinker only.
- MEM-CHK-121 vérifier no_std constraints dans tout module.
- MEM-CHK-122 vérifier absence std imports accidentels.
- MEM-CHK-123 vérifier cfg(test) sections isolées.
- MEM-CHK-124 vérifier compile target bare-metal.
- MEM-CHK-125 vérifier UB potentielle pointeurs bruts.
- MEM-CHK-126 vérifier `unsafe` avec commentaire SAFETY.
- MEM-CHK-127 vérifier memset/memcpy physmap bounds.
- MEM-CHK-128 vérifier endian assumptions explicites.
- MEM-CHK-129 vérifier integer overflow checked/wrapping justifié.
- MEM-CHK-130 vérifier `usize` conversions sûres.
- MEM-CHK-131 vérifier saturating arithm where needed.
- MEM-CHK-132 vérifier copy loops vectorisées safe.
- MEM-CHK-133 vérifier DMA descriptors alignement cacheline.
- MEM-CHK-134 vérifier cache flush/invalidate sequences.
- MEM-CHK-135 vérifier IOMMU faults propagated.
- MEM-CHK-136 vérifier fallback SW copy si DMA indisponible.
- MEM-CHK-137 vérifier shared pools contamination cross-core.
- MEM-CHK-138 vérifier lock poisoning non pertinent no_std.
- MEM-CHK-139 vérifier panic paths minimalistes.
- MEM-CHK-140 vérifier alloc_error handling complet.
- MEM-CHK-141 vérifier emergency allocations reserved paths.
- MEM-CHK-142 vérifier memory pressure hooks branch-free.
- MEM-CHK-143 vérifier metadata caches memory footprints.
- MEM-CHK-144 vérifier red zones debug only.
- MEM-CHK-145 vérifier KASAN-lite overhead acceptable.
- MEM-CHK-146 vérifier THP mode rwlock contention.
- MEM-CHK-147 vérifier THP promote race handling.
- MEM-CHK-148 vérifier THP split race handling.
- MEM-CHK-149 vérifier hugetlb reserve accounting.
- MEM-CHK-150 vérifier page flags transitions valid.
- MEM-CHK-151 vérifier user/kernel permission separation.
- MEM-CHK-152 vérifier SMAP copy windows exactly scoped.
- MEM-CHK-153 vérifier PKU keys cleanup lifecycle.
- MEM-CHK-154 vérifier canary per-thread not leaked.
- MEM-CHK-155 vérifier guard pages around stacks.
- MEM-CHK-156 vérifier leak detector hooks optional.
- MEM-CHK-157 vérifier alignment macros unified.
- MEM-CHK-158 vérifier trait object usage minimal hot-path.
- MEM-CHK-159 vérifier static mut minimisés.
- MEM-CHK-160 vérifier cacheline false-sharing risks.
- MEM-CHK-161 vérifier per-cpu stats layout.
- MEM-CHK-162 vérifier metrics update Relaxed appropriate.
- MEM-CHK-163 vérifier Acquire on read side where needed.
- MEM-CHK-164 vérifier Release on publish side where needed.
- MEM-CHK-165 vérifier SeqCst uniquement si indispensable.
- MEM-CHK-166 vérifier TLB queue memory ordering.
- MEM-CHK-167 vérifier shootdown ack visibility.
- MEM-CHK-168 vérifier CR3 updates synchronized.
- MEM-CHK-169 vérifier address canonical checks.
- MEM-CHK-170 vérifier virtual gaps no overlap kernel regions.
- MEM-CHK-171 vérifier vmalloc fragmentation stats.
- MEM-CHK-172 vérifier vmalloc free coalescing.
- MEM-CHK-173 vérifier heap large path free detection.
- MEM-CHK-174 vérifier small alloc fallback on slub fail.
- MEM-CHK-175 vérifier size_classes mapping exact.
- MEM-CHK-176 vérifier zeroed alloc semantics.
- MEM-CHK-177 vérifier nonzeroed alloc semantics.
- MEM-CHK-178 vérifier cache warmup strategy.
- MEM-CHK-179 vérifier boot memory map trust boundaries.
- MEM-CHK-180 vérifier reserved regions never allocated.
- MEM-CHK-181 vérifier ACPI reclaimable handling.
- MEM-CHK-182 vérifier bad memory regions skipped.
- MEM-CHK-183 vérifier DMA32 constraints respectées.
- MEM-CHK-184 vérifier highmem logic inert on 64-bit.
- MEM-CHK-185 vérifier movable zone migration semantics.
- MEM-CHK-186 vérifier reclaim isolates pinned pages.
- MEM-CHK-187 vérifier swap skip pinned pages.
- MEM-CHK-188 vérifier swap skip DMA pages.
- MEM-CHK-189 vérifier zswap pool bounds.
- MEM-CHK-190 vérifier zswap compression header.
- MEM-CHK-191 vérifier zswap decompression errors.
- MEM-CHK-192 vérifier OOM victim selection deterministic.
- MEM-CHK-193 vérifier oom score tie-break stable.
- MEM-CHK-194 vérifier shrinker registration id lifetime.
- MEM-CHK-195 vérifier unregister safe while running.
- MEM-CHK-196 vérifier futex waiter list consistency.
- MEM-CHK-197 vérifier waiter wake reason correctness.
- MEM-CHK-198 vérifier spurious wake semantics documented.
- MEM-CHK-199 vérifier cancellation idempotente.
- MEM-CHK-200 vérifier requeue semantics fairness.
- MEM-CHK-201 vérifier lock ordering in requeue src<dst.
- MEM-CHK-202 vérifier debug asserts suffisants.
- MEM-CHK-203 vérifier release builds sans asserts critiques.
- MEM-CHK-204 vérifier docs API à jour.
- MEM-CHK-205 vérifier docs TODO/stub à jour.
- MEM-CHK-206 vérifier naming cohérence modules.
- MEM-CHK-207 vérifier publics minimisés.
- MEM-CHK-208 vérifier private helpers non exportés.
- MEM-CHK-209 vérifier trait bounds Send/Sync corrects.
- MEM-CHK-210 vérifier unsafe impl Send/Sync justifiés.
- MEM-CHK-211 vérifier pointer provenance commentaires.
- MEM-CHK-212 vérifier provenance conversions phys->virt.
- MEM-CHK-213 vérifier aliasing rules respectées.
- MEM-CHK-214 vérifier direct map accesses clôturés.
- MEM-CHK-215 vérifier user copy ne fuit pas kernel data.
- MEM-CHK-216 vérifier zero_user jamais partiel silencieux.
- MEM-CHK-217 vérifier fault handler retours exhaustifs.
- MEM-CHK-218 vérifier segfault vs oom distinction.
- MEM-CHK-219 vérifier kernel fault panic path.
- MEM-CHK-220 vérifier path stats reset function.
- MEM-CHK-221 vérifier tests unitaires clés existent.
- MEM-CHK-222 vérifier tests intégration mmap/fault existent.
- MEM-CHK-223 vérifier test THP promote/split.
- MEM-CHK-224 vérifier test swap in/out.
- MEM-CHK-225 vérifier test futex contention.
- MEM-CHK-226 vérifier test IOMMU mapping.
- MEM-CHK-227 vérifier test DMA fallback SW.
- MEM-CHK-228 vérifier test OOM killer trigger.
- MEM-CHK-229 vérifier test shrinker callback.
- MEM-CHK-230 vérifier test NUMA policy.
- MEM-CHK-231 vérifier test page table walk.
- MEM-CHK-232 vérifier test COW break.
- MEM-CHK-233 vérifier test TLB shootdown multi-core.
- MEM-CHK-234 vérifier test alloc/free fuzz.
- MEM-CHK-235 vérifier test vma tree invariants.
- MEM-CHK-236 vérifier test mprotect transitions.
- MEM-CHK-237 vérifier test brk growth/shrink.
- MEM-CHK-238 vérifier test vmalloc boundaries.
- MEM-CHK-239 vérifier test guard pages trap.
- MEM-CHK-240 vérifier test canary violation path.
- MEM-CHK-241 vérifier test sanitizer shadow map.
- MEM-CHK-242 vérifier test swap compress/decompress.
- MEM-CHK-243 vérifier test zone allocator fallback.
- MEM-CHK-244 vérifier test emergency pool exhaustion.
- MEM-CHK-245 vérifier plan de remédiation TODO >64 CPUs.
- MEM-CHK-246 vérifier plan de remédiation ARM SMMU.
- MEM-CHK-247 vérifier plan de remédiation NUMA stub.
- MEM-CHK-248 vérifier roadmap refonte incrémentale memory.
- MEM-CHK-249 vérifier backlog bugs priorisés criticité.
- MEM-CHK-250 vérifier matrice ownership fichiers.
- MEM-CHK-251 vérifier stratégie de migration API sans rupture.
- MEM-CHK-252 vérifier compatibilité syscall ABI mémoire.
- MEM-CHK-253 vérifier sécurité side-channel baseline.
- MEM-CHK-254 vérifier métriques perf de référence.
- MEM-CHK-255 vérifier budget latence allocs hot-path.
- MEM-CHK-256 vérifier budget latence page-fault path.
- MEM-CHK-257 vérifier budget latence TLB shootdown.
- MEM-CHK-258 vérifier budget latence futex wake path.
- MEM-CHK-259 vérifier readiness globale pour refonte.
- MEM-CHK-260 vérifier clôture audit avec plan d’actions.

---

## 9) Conclusion

Le module `memory` est vaste, dense, et déterminant.
La refonte doit être découpée par sous-système.
Priorité immédiate: chemins VM/fault/TLB/futex.
Priorité secondaire: stubs NUMA/ARM IOMMU.
Priorité continue: tests de concurrence et non-régression.
