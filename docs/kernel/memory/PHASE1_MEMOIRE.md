# Phase 1 — Mémoire virtuelle et heap kernel

**Prérequis exo-boot · Modules : `memory/virtual/`, `memory/heap/`, `memory/swap/compress.rs`**

> **Phase bloquante absolue.** Rien d'autre ne peut avancer sans la heap kernel.  
> Condition de sortie : heap kernel opérationnelle, `alloc::vec::Vec` utilisable dans le kernel,  
> `map_mmio_region()` disponible pour l'APIC et le HPET.

**✅ PHASE 1 COMPLÈTE À 100% — build OK, vga_early intégré, probes nettoyées**

**✅ Phase 1 — COMPLÈTE à 100% (build OK, vga_early intégré, probes nettoyées)**

---

## 1. Vue d'ensemble

Phase 1 recouvre l'intégralité de la couche mémoire (Couche 0 — aucune dépendance scheduler/process/IPC/FS) :

| Sous-système | Rôle | Chemin principal |
|---|---|---|
| Physmap / page tables | PML4 kernel, flags PTE, KPTI split | `arch/x86_64/paging.rs`, `memory/virtual/page_table/` |
| Allocateur physique | EmergencyPool → Bitmap → Buddy | `memory/physical/` |
| Heap kernel | SLUB + large (vmalloc) → hybrid dispatch | `memory/heap/` |
| VMA tree | Arbre AVL des VMAs par espace d'adressage | `memory/virtual/vma/` |
| Protections hardware | NX, SMEP, SMAP, PKU, KPTI | `memory/protection/`, `arch/x86_64/spectre/` |
| Swap compress | zswap pool LZ4-lite, retard I/O swap | `memory/swap/compress.rs` |
| APIC MMIO UC | Remappage LAPIC/IOAPIC avec attributs corrects | `arch/x86_64/paging.rs` + `arch/x86_64/apic/` |

---

## 2. Séquence de boot — `arch_boot_init()`

**Fichier** : `kernel/src/arch/x86_64/boot/early_init.rs`

Le premier code Rust exécuté après le passage en mode long 64 bits. Sortie debug sur le port `0xE9`
(QEMU ISA debugcon) — chaque étape émet un octet :

```
Étape 1  '1' — Détection fonctionnalités CPU (SSE2, SYSCALL, XSAVE, TSC-Deadline…)
Étape 2  '2' — GDT per-CPU BSP
Étape 3  '3' — IDT (Interrupt Descriptor Table)
Étape 4        TSS per-CPU BSP + IST stacks (fait dans init_gdt_for_cpu)
Étape 5  '5' — Per-CPU data / GSBASE (init_percpu_for_bsp)
Étape 6  '6' — TSC (init_tsc — valeur initiale, calibration différée)
Étape 7  '7' — FPU / SSE / AVX (init_fpu_for_cpu — CR0.MP/EM/TS)
Étape 8  '8' — Détection hyperviseur (CPUID leaf 0x40000000)
Étape 9  '9' — ACPI : RSDP → MADT → HPET → PM Timer
Étape 10 'a' — APIC : init_apic_system() + I/O APIC (si MADT présent)
Étape 11 'b' — calibrate_lapic_timer() (mesure LAPIC tick via TSC)
Étape 12 'c' — init_memory_integration() (IPI TLB sender, arch ↔ memory)
Étape 13 'd' — init_syscall() (MSR LSTAR/STAR/SFMASK)
Étape 14 'e' — apply_mitigations_bsp() (IBRS, SSBD, retpoline)
Étape 15 'f' — Parse Multiboot2 / BootInfo exo-boot → init sous-système mémoire physique
Étape 16 'g' — SMP : boot des APs (si cpu_count > 1)
               'Z' — arch_boot_init() terminé
```

**Séquence output de référence validée** :
```
XK12356789abcdefgZAI
OK
```
(RC=124 — `head -c 8192` timeout attendu, pas d'erreur)

**Invariants d'entrée** :
- Mode long 64 bits actif, interruptions OFF (`EFLAGS.IF = 0`)
- Paging identité 1:1 sur les 4 premiers GiB (trampoline bootloader)
- GDT temporaire bootstrap chargée, IDT vide

---

## 3. Initialisation mémoire physique

**Fichier** : `kernel/src/arch/x86_64/boot/memory_map.rs`

### 3.1 Séquence en 4 phases (règle MEM-02)

```
Précondition : EmergencyPool initialisé EN PREMIER (RÈGLE EMERGENCY-01)
               kernel_init() ligne 1 : emergency_pool::init()

Phase 1 — init_phase1_bitmap(phys_start, phys_end)
    Bitmap binaire de tous les frames physiques (1 bit/frame = 512 KiB pour 4 GiB).
    Utilisé comme allocateur temporaire avant que le buddy ne soit prêt.

Phase 2 — init_phase2_free_region(base, end)  [appelé par zones E820/UEFI libres]
    Marque les régions libres dans le bitmap.
    Les régions ACPI NVS, AcpiReclaimable, Reserved, Bad sont EXCLUES (MEM-02 ✅).

Phase 2b — init_phase2b_buddy_zone(zone_type, provider) + free_region()
    Initialise le buddy allocator (zone DMA32, < 4 GiB).
    Peuple le buddy avec les mêmes régions libres que le bitmap.
    Physmap requise opérationnelle (mappée par le trampoline).

Phase 2.5 — register_slab_page_provider(provider)
    Enregistre le fournisseur de pages physiques pour le slab/SLUB.
    Doit être fait AVANT init_phase3_slab_slub().

Phase 3 — init_phase3_slab_slub()
    Initialise les caches SLUB pour chaque classe de taille.
    Active tous les SLUB_CACHES via cache.enable().

Phase 4 — init_phase4_numa(nodes_mask)
    Topologie NUMA minimale (nœud 0 par défaut si ACPI SRAT absent).
```

### 3.2 Buddy Allocator

**Fichier** : `kernel/src/memory/physical/allocator/buddy.rs`

Principe : blocs de taille $2^{\text{order}} \times \text{PAGE\_SIZE}$, free-lists par ordre.

| Propriété | Valeur |
|---|---|
| Algorithme | Buddy system classique O(log n) |
| Fusion (coalescing) | Récursive — buddy = `P ⊕ (1 << order)` |
| Alignement | Bloc d'ordre `k` : adresse alignée sur $2^k$ pages |
| Zones | DMA (< 16 MiB), DMA32 (< 4 GiB), Normal (≥ 4 GiB) |
| Fragmentation | Nulle en interne — max 1 bloc par ordre non fusionné |

**FreeNode** (embedding in free block) :
```rust
#[repr(C)]
struct FreeNode {
    next:  *mut FreeNode,  // liste circulaire doublement chaînée
    prev:  *mut FreeNode,
    order: u8,
    _pad:  [u8; 7],
}
```

### 3.3 EmergencyPool

**Fichier** : `kernel/src/memory/physical/frame/emergency_pool.rs`

Pool statique de frames pré-allouées, utilisé dans les chemins ISR/No-Alloc (RÈGLE SCHED-08).  
Initialisé **avant tout** autre allocateur — sinon les wait queues du scheduler crashent au wake.

---

## 4. Flags de page mémoire (PTE x86_64)

**Fichier** : `kernel/src/arch/x86_64/paging.rs`

| Constante | Valeur | Usage |
|---|---|---|
| `PTE_PRESENT` | bit 0 | Page présente |
| `PTE_WRITABLE` | bit 1 | Lecture/écriture |
| `PTE_USER` | bit 2 | Accessible Ring 3 |
| `PTE_WRITE_THROUGH` | bit 3 | Write-Through cache |
| `PTE_CACHE_DISABLE` | bit 4 | Cache désactivé (PCD) |
| `PTE_HUGE` | bit 7 | 2 MiB (PD) / 1 GiB (PDPT) |
| `PTE_GLOBAL` | bit 8 | Non flushed sur CR3 switch (PGE) |
| `PTE_COW` | bit 9 | Copy-on-Write pending (Exo-OS) |
| `PTE_SHM_PINNED` | bit 10 | Shared memory pinné (Exo-OS) |
| `PTE_NO_EXEC` | bit 63 | NX / XD bit |

**Combinaisons pré-définies** :

| Constante | Flags | Usage |
|---|---|---|
| `PAGE_FLAGS_KERNEL_RW` | Present \| Writable \| Global \| **NX** | Data kernel |
| `PAGE_FLAGS_KERNEL_RX` | Present \| Global | Code kernel (exécutable) |
| `PAGE_FLAGS_KERNEL_RO` | Present \| Global \| NX | Data kernel read-only |
| `PAGE_FLAGS_USER_RW` | Present \| Writable \| User \| NX | Data userspace |
| `PAGE_FLAGS_USER_RX` | Present \| User | Code userspace |
| `PAGE_FLAGS_USER_COW` | Present \| User \| CoW \| NX | Page CoW read-only |
| **`PAGE_FLAGS_MMIO`** | Present \| Writable \| **CacheDisable** \| **NX** | MMIO (LAPIC, IOAPIC, HPET) |

---

## 5. Remappage APIC MMIO (MEM-01)

**Fichier** : `kernel/src/arch/x86_64/apic/local_apic.rs` + `arch/x86_64/paging.rs`

### 5.1 Problème (MEM-01 — résolu ✅)

> *"Les pages APIC MMIO sont actuellement mappées P|R/W|PS sans NX et sans UC.
> L'APIC est une page de 4 KiB, jamais un huge page — PS invalide pour MMIO.
> Les registres APIC nécessitent strong uncached (UC) — sinon le CPU peut
> réordonner les lectures/écritures, causant des interruptions fantômes."*

**État corrigé** : `PAGE_FLAGS_MMIO = PTE_PRESENT | PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXEC`

- `PTE_CACHE_DISABLE` (bit 4 = PCD) + `PTE_WRITE_THROUGH` absents dans le trampoline → désormais
  intégrés dans `PAGE_FLAGS_MMIO`
- `PTE_NO_EXEC` (bit 63) ajouté — MMIO non-exécutable
- Mapping 4 KiB (pas huge page 2 MiB) — conforme aux exigences APIC

### 5.2 LAPIC xAPIC vs x2APIC

| Mode | Accès | Sélection |
|---|---|---|
| **xAPIC** | MMIO 0xFEE00000 → `LAPIC_BASE` (AtomicUsize) | défaut si CPUID~x2APIC~ absent |
| **x2APIC** | MSR 0x800–0xBFF | si `cpu_features().has_x2apic()` |

```
init_apic_system() :
  1. CPUID → has_x2apic() ?
  2. Si oui : enable_x2apic()  — écriture MSR_IA32_APICBASE bit EXTD
  3. Sinon  : enable_xapic()   — MMIO via LAPIC_BASE
  4. set_spurious_vector(VEC_SPURIOUS = 0xFF) + soft-enable (SIVR bit 8)
  5. TSC-Deadline si disponible → timer_init_tsc_deadline()
     Sinon one-shot → timer_init_oneshot()
```

### 5.3 Lectures/écritures LAPIC

```rust
// SAFETY: base validée à l'init, registre est un offset connu
pub fn lapic_read(reg: u32) -> u32 {
    unsafe { read_volatile((LAPIC_BASE + reg) as *const u32) }
}
pub fn lapic_write(reg: u32, val: u32) {
    unsafe { write_volatile((LAPIC_BASE + reg) as *mut u32, val); }
}
```

Toutes les lectures/écritures sont `read_volatile`/`write_volatile` — le compilateur
ne peut pas réordonner ni éliminer ces accès.

---

## 6. Heap kernel — Allocateur hybride

**Fichier** : `kernel/src/memory/heap/allocator/hybrid.rs`

### 6.1 Architecture

```
alloc(size, align, flags)
    ┌─ size ≤ 2048 → SLUB_CACHES[class_idx].alloc()
    └─ size > 2048 → memory::heap::large::vmalloc::kalloc()
```

**Appel d'init** (dans `kernel_init()`) :
```rust
crate::memory::heap::allocator::hybrid::init();
// → HYBRID_ENABLED.store(true, Ordering::Release)
```

Avant cet appel, toute allocation retourne `AllocError::NotInitialized`.

### 6.2 Statistiques

| Compteur | Description |
|---|---|
| `small_allocs` / `small_frees` | Objets SLUB (≤ 2 KiB) |
| `large_allocs` / `large_frees` | vmalloc (> 2 KiB) |
| `oom_count` | Échecs d'allocation totaux depuis le boot |
| `current_inuse` | Octets heap actuellement alloués |

---

## 7. SLUB — fix fat pointer vtable

**Fichier** : `kernel/src/memory/physical/allocator/slab.rs`

### 7.1 Problème

Le trait `SlabPageProvider` (interface entre slab et buddy) était utilisé via `dyn Trait` dans un `static`.  
En Rust `no_std`, un fat pointer `*const dyn Trait` stock dans un `static mut` est un pointeur  
pendouillant car la vtable vit dans `.rodata` mais la référence au `static` peut être uninitialized.

### 7.2 Solution — `DefaultProvider` (deux `AtomicUsize`)

```rust
pub struct DefaultProvider {
    data:   AtomicUsize,   // partie 1 du fat pointer (ptr vers données)
    vtable: AtomicUsize,   // partie 2 du fat pointer (ptr vers vtable)
}
```

Enregistrement via `transmute` atomique :
```rust
pub unsafe fn register_slab_page_provider(provider: *const dyn SlabPageProvider) {
    let (data, vtable): (usize, usize) = core::mem::transmute(provider);
    SLAB_PAGE_PROVIDER.data.store(data,   Ordering::SeqCst);
    SLAB_PAGE_PROVIDER.vtable.store(vtable, Ordering::SeqCst);
}
```

Reconstruction sécurisée au moment de l'appel :
```rust
fn get_page(&self) -> Result<PhysAddr, AllocError> {
    let data   = self.data.load(Ordering::Acquire);
    let vtable = self.vtable.load(Ordering::Acquire);
    if data == 0 || vtable == 0 { return Err(AllocError::NotInitialized); }
    let fat: *const dyn SlabPageProvider =
        unsafe { core::mem::transmute((data as *const (), vtable as *const ())) };
    unsafe { (*fat).get_page() }
}
```

**Ordre d'appel impératif** :
```
register_slab_page_provider()   ← Phase 2.5
init_phase3_slab_slub()         ← Phase 3
hybrid::init()                  ← kernel_init() Phase 2b
```

### 7.3 Freelist XOR-encoded (SLUB)

Chaque objet libre dans un slub préfixe un pointeur XOR-encodé :
```
ptr_to_store = real_next_ptr ⊕ freelist_key
```
Clé par slub — résistance basique aux attaques use-after-free (corruption de
freelist redirigée vers une adresse arbitraire).

---

## 8. VMA Tree (arbre AVL)

**Fichier** : `kernel/src/memory/virtual/vma/tree.rs`

```
VmaTree :
  root: *mut VmaDescriptor   (null si vide)
  count: usize
  MAX_VMAS_PER_PROCESS = 65536
```

| Opération | Complexité | Détail |
|---|---|---|
| `find(addr)` | O(log n) | Parcours AVL — retourne VMA contenant l'adresse |
| `insert(vma)` | O(log n) | Insertion + rééquilibrage AVL |
| `remove(vma)` | O(log n) | Suppression + rééquilibrage |
| `find_gap(size)` | O(log n + n) | Espace libre entre VMAs |

Le tree ne **possède** pas les descripteurs — le `VmaAllocator` gère l'ownership.  
Les nœuds sont alloués par le slab kernel.

---

## 9. Protections hardware mémoire

**Fichier** : `kernel/src/memory/protection/mod.rs`

Appelé depuis `early_init.rs` après l'init complète du sous-système mémoire :

```
protection::init() sur BSP + chaque AP :
  1. nx::init()    — EFER.NXE=1  (NX/XD bit activé dans tous les PTEs)
  2. smep::init()  — CR4.SMEP=1  (interdit l'exécution de pages user depuis Ring 0)
  3. smap::init()  — CR4.SMAP=1  (interdit l'accès aux pages user depuis Ring 0 sans STAC/CLAC)
  4. pku::init()   — CR4.PKE=1   (Protection Keys User — 16 domaines d'accès)
```

**Clés PKU pré-définies** :

| Clé | Constante | Usage |
|---|---|---|
| 0 | `PKU_DEFAULT_KEY` | Pages standards (accès complet) |
| 1 | `PKU_KERNEL_HEAP_KEY` | Heap kernel (inaccessible depuis Ring 3) |
| 2 | `PKU_GUARD_KEY` | Guard pages (aucun accès) |
| 3 | `PKU_MMIO_KEY` | Régions MMIO (accès kernel uniquement) |

---

## 10. KPTI — Kernel Page-Table Isolation

**Fichier** : `kernel/src/arch/x86_64/spectre/kpti.rs`

Mitigation Meltdown : chaque thread possède deux CR3 :

| CR3 | PCID | Contenu |
|---|---|---|
| `cr3_kernel` | `PCID_KERNEL = 0` | PML4 complète — code + data kernel + user |
| `cr3_user` | `PCID_USER = 1` | PML4 allégée — uniquement stubs syscall/exception |

Le switch se fait dans `switch_asm.s` (Phase 2 — context switch) :
```
PUSH registres
MOV CR3, cr3_user ou cr3_kernel   ← avant restauration registres (KPTI correct)
POP registres
```

> Si PCID (Process Context Identifier) est supporté, le bit 63 du CR3 (`CR3_NO_FLUSH_BIT`)
> permet de ne pas invalider le TLB lors des switches fréquents.

---

## 11. Swap compress (zswap)

**Fichier** : `kernel/src/memory/swap/compress.rs`

Pool de pages compressées en RAM pour retarder/éviter les I/O swap physiques.

| Paramètre | Valeur | Signification |
|---|---|---|
| `ZSWAP_SLOT_SIZE` | 3072 octets | Taille max d'un slot compressé (75 % de PAGE_SIZE) |
| `MAX_ZSWAP_SLOTS` | 4096 | Capacité du pool (16 MiB de slots comprés max) |
| Algorithme | LZ77-lite | Deux types de tokens : literal (1..128 octets) et match (offset 13 bits, len 3–10) |

**Règle de rejet** : si la page compressée dépasse `ZSWAP_SLOT_SIZE` (non-compressible),
elle est renvoyée vers le swap device classique sans entrée dans le pool.

---

## 12. État d'implémentation Phase 1

> **Phase 1 complète à 100%.** Build `cargo build` OK (32s). Affichage VGA boot actif. Probes parasites supprimées.

### 12.1 Checklist (tirée du roadmap)

| # | Item | État | Détail |
|---|------|------|--------|
| ✅ | PML4 kernel haute mémoire | **Implémenté** | Trampoline bootloader → physmap opérationnel |
| ✅ | APIC MMIO avec attributs corrects (UC + NX) | **Implémenté** | `PAGE_FLAGS_MMIO` — MEM-01 corrigé |
| ✅ | Buddy allocator opérationnel | **Implémenté** | 4 phases, zones DMA/DMA32/Normal |
| ✅ | SLUB allocator (`#[global_allocator]`) | **Implémenté** | fat ptr vtable fix, freelist XOR |
| ✅ | hybrid::init() heap kernel active | **Implémenté** | ≤ 2 KiB → SLUB, > 2 KiB → vmalloc |
| ✅ | VMA tree (AVL) | **Implémenté** | MAX_VMAS=65536, O(log n) |
| ✅ | Swap compress LZ4 | **Implémenté** | zswap pool 4096 slots, LZ77-lite |
| ✅ | Protections NX / SMEP / SMAP / PKU | **Implémenté** | Activées après memory init |
| ✅ | KPTI (Meltdown mitigation) | **Implémenté** | cr3_kernel + cr3_user, PCID 0+1 |
| ✅ | VGA early display (`vga_early.rs`) | **Implémenté** | 80×25 texte, identity map 0xB8000, `boot_screen()` + `boot_complete()` |
| ✅ | Probes debug port 0xE9 nettoyées | **Fait** | Séquence officielle K/A/I conservée ; parasites 'p'/'s' supprimés de `tsc.rs` |
| ✅ | Makefile debugcon | **Fait** | `-debugcon file:/tmp/e9k.txt` — capture port 0xE9 dans QEMU |
| ⚠️ | HPET MMIO remappé avec UC | **Différé** | Init différée — accès via identity map 1 GiB au boot (Phase 3) |
| ⚠️ | TSC calibré via HPET | **Différé** | Fallback 1 GHz (ERR-01) — calibration réelle Phase 2/3 |

### 12.2 Erreurs silencieuses — état de résolution

| ID | Description | État |
|---|---|---|
| `MEM-01` | APIC MMIO sans NX et sans UC → interruptions fantômes | ✅ Corrigé — `PAGE_FLAGS_MMIO` avec PCD+NX |
| `MEM-02` | Buddy alloue dans régions ACPI NVS/Reserved → corruption tables ACPI | ✅ Corrigé — types AcpiNvs/AcpiReclaimable/Reserved exclus |
| `MEM-03` | TSC fallback 3 GHz → délais faux sur hardware réel | ⚠️ Encore actif — calibration HPET Phase 2 (ERR-01) |
| `SLUB-vtable` | Fat pointer `dyn Trait` dans `static mut` → pointeur pendouillant | ✅ Corrigé — `DefaultProvider` stocke data+vtable séparément |

---

## 13. TODOs bloquants avant Phase 6 (exo-boot)

| Priorité | Action | Module | Règle |
|---|---|---|---|
| ⚠️ Correctness hw | Calibration TSC via HPET ou PM Timer ACPI | `scheduler/timer/clock.rs` + `arch/x86_64/acpi/` | `ERR-01` |
| ⚠️ Correctness hw | HPET MMIO remappé avec `PAGE_FLAGS_MMIO` post-init | `arch/x86_64/acpi/hpet.rs` | `MEM-01` partiel |
| 🔵 Futur | NUMA SRAT parsing pour affinité physique cross-socket | `memory/physical/numa/` | Phase 2.11 |
| 🔵 Futur | Huge pages (THP) pour code kernel > 2 MiB | `memory/huge_pages/thp.rs` | Performance |

---

## 14. Dépendances inter-phases

```
Phase 0 : bootloader (trampoline ASM, identity map 1:1 4 GiB, passage mode long)
    ↓
Phase 1 : Mémoire (cette phase)
    ├── EmergencyPool (initalisé dès early_init avant tout)
    ├── hybrid::init() → SLUB + vmalloc actifs → alloc::vec::Vec disponible
    ├── PAGE_FLAGS_MMIO → APIC/HPET accessibles correctement
    └── protection::init() → NX+SMEP+SMAP+PKU actifs
    ↓
Phase 2 : Scheduler + IPC (requis : heap kernel + physmap opérationnels)
Phase 3 : Process (requis : scheduler + VMA tree)
Phase 4 : ExoFS (requis : VMA mmap, swap compress, buddy)
```
