# Intégration Arch ↔ Memory — x86_64

> **Statut :** Implémenté et opérationnel  
> **Dernière mise à jour :** 22 février 2026  
> **Règles DOC2 appliquées :** MEM-01, MEM-02, MEM-04, TLB-01

---

## 1. Vue d'ensemble

L'intégration entre `arch/x86_64/` et `memory/` repose sur trois fichiers-ponts et la modification du gestionnaire d'exceptions.

```
arch/x86_64/
├── memory_iface.rs          ← pont principal (CPU primitives + KernelFaultAllocator + IPI sender)
└── boot/
    └── memory_map.rs        ← traduction carte mémoire boot → init memory/

memory/
└── arch_iface.rs            ← types physiques unifiés (E820/UEFI) + init_from_regions()

arch/x86_64/
└── exceptions.rs            ← do_page_fault + do_ipi_tlb_shootdown câblés
```

### Règle MEM-01 (Couche 0)

`memory/` **peut** importer `arch/` pour les instructions ASM bas niveau. L'inverse
est interdit afin de préserver la hiérarchie de couches. La dépendance circulaire pour
l'IPI TLB est rompue via un **function-pointer** (`register_tlb_ipi_sender`).

---

## 2. Fichiers créés

### 2.1 `arch/x86_64/memory_iface.rs`

**Rôle** : pont bidirectionnel arch ↔ memory — point unique d'intégration.

#### Primitives CPU exportées vers `memory/`

| Fonction | Description |
|---|---|
| `read_cr2() -> u64` | Adresse virtuelle du dernier `#PF` |
| `flush_cr3()` | Reload CR3 (flush TLB non-global) |
| `switch_cr3(pml4_phys: u64)` | Changement d'espace d'adressage |
| `flush_tlb_single(addr: u64)` | `INVLPG` sur une adresse |
| `flush_tlb_range(start, end)` | `INVLPG` sur une plage |
| `flush_tlb_global()` | Toggle CR4.PGE — flush entrées globales |

Toutes ces fonctions sont `unsafe` (CPL 0 requis).

#### `send_tlb_ipi_to_mask(cpu_mask: u64)` (privée)

Envoie un IPI vecteur `0xF2` à tous les CPUs référencés dans le masque 64 bits.

```rust
unsafe fn send_tlb_ipi_to_mask(cpu_mask: u64) {
    let current = percpu::current_cpu_id() as usize;
    for cpu_idx in 0..64usize {
        if cpu_mask & (1u64 << cpu_idx) == 0 { continue; }
        if cpu_idx == current || cpu_idx >= MAX_CPUS { continue; }
        let lapic_id = percpu::per_cpu(cpu_idx).lapic_id as u8;
        local_apic::send_ipi(lapic_id, IPI_TLB_SHOOTDOWN_VECTOR, ICR_DM_FIXED);
    }
}
```

- Le CPU courant est **exclu** : son flush local a déjà eu lieu (règle TLB-01).
- CPUs hors-ligne ignorés via `cpu_idx >= MAX_CPUS`.

#### `init_memory_integration()` (publique)

Enregistre `send_tlb_ipi_to_mask` auprès de `memory::virt::address_space::tlb::register_tlb_ipi_sender()`.

**Quand appeler :** après `apic::init_apic_system()`, avant `smp::smp_boot_aps()`.  
**Idempotente** : flag `AtomicBool` protège contre les appels multiples.

#### `KernelFaultAllocator`

Struct sans état implémentant `FaultAllocator` + `FrameAllocatorForWalk` pour le
gestionnaire de fautes de page kernel.

| Méthode | Implémentation |
|---|---|
| `alloc_frame(flags)` | `alloc_page(flags)` |
| `free_frame(f)` | `free_page(f)` |
| `alloc_zeroed()` | `alloc_page(AllocFlags::ZEROED)` |
| `alloc_nonzeroed()` | `alloc_page(AllocFlags::NONE)` |
| `map_page(virt, frame, flags)` | `unsafe { KERNEL_AS.map(...) }` |
| `remap_flags(virt, flags)` | `PageTableWalker::new(pml4).remap_flags(...)` |
| `translate(virt)` | `KERNEL_AS.translate(virt)` |

Instance globale : `pub static KERNEL_FAULT_ALLOC: KernelFaultAllocator`

---

### 2.2 `arch/x86_64/boot/memory_map.rs`

**Rôle** : traduire la carte mémoire E820 (Multiboot2) ou UEFI en appels vers les
phases d'initialisation de `memory/physical/allocator/`.

#### Fonctions principales

```
init_memory_subsystem_multiboot2(info: &Multiboot2Info)
init_memory_subsystem_uefi(uefi_map: &UefiMemoryMap)
```

Séquence d'initialisation (identique pour les deux sources) :

```
1. init_phase1_bitmap(phys_start=1MiB, phys_end=max_addr_trouvée)
2. Pour chaque région Usable de la carte E820/UEFI :
       → init_phase2_free_region(start, end)
3. init_phase3_slab_slub()
4. init_phase4_numa(0b01)      ← nœud 0 uniquement (SMP homogène)
```

#### Types exportés

| Type | Description |
|---|---|
| `MemoryRegion` | `{ base: u64, size: u64, region_type }` |
| `MemoryRegionType` | `Usable / Reserved / AcpiReclaimable / AcpiNvs / Bad / KernelImage` |
| `MEMORY_MAP: [MemoryRegion; 256]` | Carte statique remplie au boot |
| `MEMORY_REGION_COUNT: usize` | Nombre de régions valides |
| `PHYS_MEMORY_START` | `0x0010_0000` (1 MiB — limité basse mémoire BIOS) |
| `PHYS_MEMORY_MAX` | `(1 << 48) - 1` (espace d'adressage physique x86_64 48-bit) |

---

### 2.3 `memory/arch_iface.rs`

**Rôle** : types canoniques côté `memory/` pour décrire la RAM physique indépendamment
de la source boot (E820 ou UEFI).

```rust
pub struct PhysMemoryRegion {
    pub base:        u64,
    pub size:        u64,
    pub region_type: PhysRegionType,
}

pub enum PhysRegionType {
    Usable, Reserved, AcpiReclaimable, AcpiNvs, Defective, FirmwareReserved,
}
```

#### `init_from_regions(regions: &[PhysMemoryRegion])`

Appelle les phases 1→4 de l'allocateur physique depuis un slice de régions unifié.
Utilisable depuis n'importe quel niveau de boot (Multiboot2, UEFI, DT).

#### Constantes partagées

| Constante | Valeur | Description |
|---|---|---|
| `IPI_TLB_SHOOTDOWN_VECTOR` | `0xF2` | Vecteur IDT IPI TLB |
| `IPI_RESCHEDULE_VECTOR` | `0xF1` | Vecteur IDT IPI reschedule |
| `MAX_NUMA_NODES` | `8` | Nœuds NUMA max supportés |
| `MAX_CPUS` | `512` | CPUs max (≡ `percpu::MAX_CPUS`) |

---

## 3. Modifications existantes

### 3.1 `arch/x86_64/exceptions.rs` — `do_page_fault`

```
ASM → exc_page_fault_handler → do_page_fault(frame: *mut ExceptionFrame)
```

**Flux :**

1. Lire CR2 → adresse fautive.
2. Décoder l'`error_code` → `FaultCause` (`Read / Write / Execute / Protection`).
3. Construire `FaultContext { addr, cause, from_userspace }`.
4. Appeler `handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)`.
5. Matcher `FaultResult` :

| Résultat | Action |
|---|---|
| `Handled` | Retour normal (userspace ou kernel) |
| `Segfault { addr }` | Signal SIGSEGV (futur) ou `kernel_panic_exception` |
| `Oom { addr }` | Signal SIGKILL (futur) ou kernel panic OOM |
| `KernelFault { addr }` | `kernel_panic_exception` (toujours fatal) |

### 3.2 `arch/x86_64/exceptions.rs` — `do_ipi_tlb_shootdown`

```
IPI vecteur 0xF2 → ipi_tlb_shootdown_handler → do_ipi_tlb_shootdown(frame)
```

**Flux :**

1. Incrémenter le compteur IRQ `0xF2`.
2. Incrémenter le compteur `paging::inc_tlb_shootdown()`.
3. Lire `percpu::current_cpu_id()` → `cpu_id: u8`.
4. Appeler `TLB_QUEUE.handle_remote(cpu_id)` — flush TLB + marque ACK.
5. Envoyer EOI au Local APIC.

### 3.3 `arch/x86_64/boot/early_init.rs` — Séquence de boot

**Deux insertions par rapport à la version initiale :**

**Après l'étape 10 (init APIC) :**
```rust
super::super::memory_iface::init_memory_integration();
```

**Après parse Multiboot2 (étape 13) :**
```rust
// Règle MEM-02 : EmergencyPool AVANT tout allocateur
crate::memory::physical::frame::emergency_pool::init();
// Phases 1→4 (bitmap → free_regions → slab → NUMA)
super::memory_map::init_memory_subsystem_multiboot2(&mb2);
// Enregistrer PML4 courante dans l'espace d'adressage kernel
crate::memory::virt::address_space::KERNEL_AS.init(
    crate::memory::core::types::PhysAddr::new(super::super::read_cr3()),
);
// Activer les protections hardware (NX, SMEP, SMAP, PKU)
crate::memory::protection::init();
```

### 3.4 Déclarations de modules

| Fichier | Ajout |
|---|---|
| `arch/x86_64/mod.rs` | `pub mod memory_iface;` |
| `arch/x86_64/boot/mod.rs` | `pub mod memory_map;` + re-exports |
| `memory/mod.rs` | `pub mod arch_iface;` |

---

## 4. Flux complets

### 4.1 Page Fault (#PF)

```
CPU lève #PF
    │
    ▼
exc_page_fault_handler (ASM stub, sauvegarde registres)
    │
    ▼
do_page_fault(frame)                      [arch/x86_64/exceptions.rs]
    │  read_cr2() → addr
    │  decode error_code → FaultCause
    │  build FaultContext
    ▼
handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC) [memory/virtual/fault/handler.rs]
    │  CoW ? demand paging ? protection violation ?
    │  appelle alloc/map via FaultAllocator (→ KERNEL_FAULT_ALLOC)
    ▼
FaultResult::{Handled, Segfault, Oom, KernelFault}
    │
    ▼
do_page_fault: dispatch (IRET / signal / kernel_panic)
```

### 4.2 TLB Shootdown

```
memory/ décide de libérer des frames
    │
    ▼ (règle TLB-01)
1. flush_tlb_local()         ← flush sur le CPU demandeur
2. TLB_QUEUE.shootdown(...)  ← dépose requête + appelle send_tlb_ipi(cpu_mask)
    │
    ▼ (via function-pointer enregistré par init_memory_integration)
send_tlb_ipi_to_mask(cpu_mask)           [arch/x86_64/memory_iface.rs]
    │  for cpu_idx in 0..64
    │      local_apic::send_ipi(lapic_id, 0xF2, FIXED)
    ▼
(sur chaque CPU cible, IPI arrivée)
do_ipi_tlb_shootdown()                   [arch/x86_64/exceptions.rs]
    │  TLB_QUEUE.handle_remote(cpu_id)   ← flush TLB + ACK
    │  local_apic::eoi()
    ▼
3. TLB_QUEUE.wait_for_completion()       ← memory/ attend tous les ACKs
4. free_pages(frames)                    ← libération effective (jamais avant ACK)
```

### 4.3 Boot — Init mémoire

```
arch_boot_init(mb2_magic, mb2_info, rsdp)    [boot/early_init.rs]
    │
    ├─ [Étape 10] apic::init_apic_system()
    │               memory_iface::init_memory_integration()   ← IPI TLB ready
    │
    ├─ [Étape 13] parse_multiboot2(mb2_info) → Multiboot2Info
    │               emergency_pool::init()                     ← MEM-02 FIRST
    │               init_memory_subsystem_multiboot2(&mb2)
    │                   ├─ init_phase1_bitmap(1MiB, max_phys)
    │                   ├─ init_phase2_free_region(r) × N
    │                   ├─ init_phase3_slab_slub()
    │                   └─ init_phase4_numa(0b01)
    │               KERNEL_AS.init(PhysAddr::new(read_cr3()))
    │               memory::protection::init()                 ← NX/SMEP/SMAP/PKU
    │
    └─ [Étape 14] smp_boot_aps(madt, bsp_lapic)
```

---

## 5. Invariants et garanties

| Invariant | Garantie |
|---|---|
| `EmergencyPool` init AVANT tout allocateur | Respecté — appelé en premier dans l'étape 13 (MEM-02) |
| IPI TLB enregistré AVANT boot des APs | Respecté — étape 10+, avant étape 14 |
| Frames libérées APRÈS ACK complet | Respecté — `TLB_QUEUE::wait_for_completion` appelé avant `free_pages` dans `memory/` (MEM-04) |
| flush_local AVANT IPI | Respecté — `shootdown()` dans `tlb.rs` flush local puis émet IPIs (TLB-01) |
| Pas de self-IPI TLB | Respecté — `send_tlb_ipi_to_mask` exclut le CPU courant |
| `remap_flags` toujours safe | Respecté — `PageTableWalker::remap_flags` est une fn safe, pas d'`unsafe` inutile |

---

## 6. Limitations connues

- **`KernelFaultAllocator` uniquement pour le kernel** : les page faults userspace passent
  actuellement par le même allocateur. Quand `process/` sera intégré, chaque processus
  aura son propre `FaultAllocator` lié à son `UserAddressSpace`.
- **UEFI** : `init_memory_subsystem_uefi()` est implémentée mais non câblée dans
  `early_init.rs` (chemin Multiboot2 uniquement pour l'instant ; UEFI sera ajouté quand
  le boot UEFI sera opérationnel).
- **NUMA multi-nœuds** : `init_phase4_numa(0b01)` initialise uniquement le nœud 0.
  La topologie NUMA complète sera câblée depuis `acpi/srat.rs` une fois le parser SRAT implémenté.
