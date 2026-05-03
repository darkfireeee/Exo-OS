# Exo-OS — Audit Profond & Correctifs Complets (Partie 2)
## Modules : `arch`, `memory`, `scheduler` — Suite

**Auteur :** claude-beta  
**Date :** 2026-05-03  
**Portée :** Analyse approfondie FPU/Lazy, SMP/AP, page fault, syscall entry, IOMMU, memory core  
**Référence :** Partie 1 — `EXOOS_AUDIT_CORRECTIONS_ARCH_MEMORY_SCHEDULER.md`

---

## INDEX PARTIE 2

| ID | Priorité | Module | Fichier | Résumé |
|----|----------|--------|---------|--------|
| DEF-13 | **P0** | arch / scheduler | `smp/init.rs` | `scheduler::init_ap()` jamais appelé depuis l'entrée AP → CR0.TS ne sera jamais 1 sur les APs |
| DEF-14 | **P0** | scheduler/fpu | `lazy.rs` | `FPU_LAZY_INITIALIZED` : flag par processeur globalisé — faux sur APs |
| DEF-15 | **P1** | arch | `syscall.rs` | SYSRETQ non protégé : RSP userspace restauré depuis GS sans vérification de canonicité |
| DEF-16 | **P1** | memory / fault | `handler.rs` + `exceptions.rs` | #PF handler : `FaultContext::find_vma()` retourne toujours `None` (VMA jamais fournie) |
| DEF-17 | **P1** | arch | `smp/init.rs` | Absence de LFENCE après lecture `AP_ALIVE_MAGIC` — spéculative store bypass possible |
| DEF-18 | **P1** | memory | `frame/ref_count.rs` | `inc()` : `fetch_add` avec Relaxed — pas de barrière Acquire avant première lecture du frame |
| DEF-19 | **P1** | scheduler | `core/runqueue.rs` | `nr_running_usize()` sans synchronisation — utilisé dans hot path tick pour calculer le quantum CFS |
| DEF-20 | **P2** | arch | `spectre/kpti.rs` | `kpti_switch_to_user()` : panic si `cr3_user == 0` — crash sur CPU n'ayant pas encore fait de context switch |
| DEF-21 | **P2** | memory/dma | `iommu/intel_vtd.rs` | Root Table : `zero()` utilise `write_bytes` sans `compiler_fence` → store peut être réordonnancé avant `GCMD.SRTP` |
| DEF-22 | **P2** | scheduler | `timer/tick.rs` | `ELAPSED_NS` non réinitialisé sur les APs au démarrage → première mesure de quantum erronée |

---

## DEF-13 — `scheduler::init_ap()` Jamais Appelé : CR0.TS Absent Sur Les APs (P0)

### Localisation

`kernel/src/arch/x86_64/smp/init.rs` — fonction `ap_entry()`

### Description du défaut

La séquence d'initialisation AP dans `ap_entry()` est :

```rust
pub unsafe extern "C" fn ap_entry(cpu_id: u32, lapic_id: u32, kernel_stack_top: u64) -> ! {
    percpu::init_percpu_for_ap(cpu_id, kernel_stack_top, lapic_id);
    super::super::gdt::init_gdt_for_cpu(cpu_id as usize, kernel_stack_top);
    super::super::idt::load_idt();
    super::super::syscall::init_syscall();
    super::super::apic::init_ap_local_apic();
    tsc::init_tsc(cpu_id);
    super::super::cpu::fpu::init_fpu_for_cpu();  // ← initialise la FPU hardware
    let _ = crate::scheduler::core::publish_current_boot_idle(cpu_id, kernel_stack_top);
    super::super::spectre::apply_mitigations_ap();
    // ...
    core::arch::asm!("sti", options(nostack, nomem));
    loop { core::arch::asm!("hlt", options(nostack, nomem)); }
}
```

**`crate::scheduler::init_ap(cpu_id)` n'est JAMAIS appelé.**

Or `scheduler::init_ap()` est défini dans `scheduler/mod.rs` avec une responsabilité critique :

```rust
pub unsafe fn init_ap(cpu_id: u32) {
    // CR0.TS=1 sur cet AP (FPU lazy).
    self::fpu::lazy::init();
    let _ = cpu_id;
}
```

**Conséquences :**

1. Sur chaque AP, `fpu::lazy::init()` n'est jamais appelé → `cr0_set_ts()` n'est jamais exécuté sur les APs.
2. Les APs démarrent avec `CR0.TS = 0` (valeur après `init_fpu_for_cpu()` qui appelle `clts`).
3. Tout thread scheduler sur un AP peut utiliser la FPU **sans déclencher #NM** → l'état FPU n'est jamais sauvé/restauré via `XSAVE`/`XRSTOR` lors des context switches sur ces CPUs.
4. Les context switches sur les APs corrompent silencieusement l'état FPU entre threads.
5. `FPU_LAZY_INITIALIZED` reste `false` sur les APs (voir DEF-14), ce qui masque le bug.

**Impact maximum :** tous les threads utilisant SSE/AVX sur des CPUs APs partagent des registres FPU sans isolation.

### Correction

Ajouter l'appel à `scheduler::init_ap()` dans `ap_entry()`, **après** `publish_current_boot_idle` et **avant** `STI` :

```rust
pub unsafe extern "C" fn ap_entry(cpu_id: u32, lapic_id: u32, kernel_stack_top: u64) -> ! {
    // 1. Per-CPU data
    percpu::init_percpu_for_ap(cpu_id, kernel_stack_top, lapic_id);
    // 2. GDT per-CPU + TSS
    super::super::gdt::init_gdt_for_cpu(cpu_id as usize, kernel_stack_top);
    // 3. IDT
    super::super::idt::load_idt();
    // 3b. SYSCALL/SYSRET MSRs
    super::super::syscall::init_syscall();
    // 4. LAPIC AP
    super::super::apic::init_ap_local_apic();
    // 5. TSC
    tsc::init_tsc(cpu_id);
    // 6. FPU hardware (XSAVE detection, CR0.EM=0, CR4.OSFXSR=1, etc.)
    super::super::cpu::fpu::init_fpu_for_cpu();
    // 6b. Boot idle TCB
    let _ = crate::scheduler::core::publish_current_boot_idle(cpu_id, kernel_stack_top);

    // ════ CORRECTIF DEF-13 ════════════════════════════════════════════════════
    // 6c. Initialiser le scheduler sur cet AP :
    //     - Appelle fpu::lazy::init() → cr0_set_ts() → CR0.TS = 1
    //     - Met FPU_LAZY_INITIALIZED = true sur cet AP
    // DOIT être après init_fpu_for_cpu() (qui remet CR0.TS=0 via clts).
    // DOIT être avant STI (eviter une exception #NM avec CR0.TS=0 sur cet AP).
    crate::scheduler::init_ap(cpu_id);
    // ═════════════════════════════════════════════════════════════════════════

    // 7. Mitigations spectre
    super::super::spectre::apply_mitigations_ap();
    // 8. Handshake BSP
    core::ptr::write_volatile(
        (TRAMPOLINE_PHYS + HANDSHAKE_OFFSET) as *mut u32,
        AP_ALIVE_MAGIC,
    );
    // 8b. Attendre SECURITY_READY
    while !crate::security::is_security_ready() {
        core::hint::spin_loop();
    }
    // 9. STI + boucle idle
    core::arch::asm!("sti", options(nostack, nomem));
    loop {
        core::arch::asm!("hlt", options(nostack, nomem));
    }
}
```

---

## DEF-14 — `FPU_LAZY_INITIALIZED` : Flag Global Faussement Interprété Par CPU (P0)

### Localisation

`kernel/src/scheduler/fpu/lazy.rs` — variable `FPU_LAZY_INITIALIZED` et fonction `is_initialized()`

### Description du défaut

```rust
static FPU_LAZY_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    unsafe { cr0_set_ts(); }
    FPU_LAZY_INITIALIZED.store(true, Ordering::Release);
}

pub fn is_initialized() -> bool {
    FPU_LAZY_INITIALIZED.load(Ordering::Relaxed)
}
```

`FPU_LAZY_INITIALIZED` est un `static` global partagé entre tous les CPUs. Une fois que le BSP a appelé `init()`, le flag devient `true` pour **tous les CPUs** — même ceux qui n'ont pas encore appelé `init()` et sur lesquels `CR0.TS` n'est pas encore à 1.

Cette confusion est aggravée par DEF-13 : les APs ne s'appellent jamais `fpu::lazy::init()`, donc même si le flag était per-CPU, il resterait `false` sur les APs.

**Tout code vérifiant `is_initialized()` pour conditionner un comportement FPU sera trompé sur les APs.**

### Correction

**Option A (correcte si DEF-13 est corrigé) :** Lire l'état réel de `CR0.TS` plutôt que le flag global.

```rust
/// Retourne vrai si le Lazy FPU est opérationnel sur LE CPU COURANT.
///
/// Implémentation : lit CR0.TS directement plutôt qu'un flag global faillible.
/// CR0.TS = 1 signifie que l'initialisation Lazy FPU est en place sur ce CPU.
/// Note : nécessite Ring 0.
#[inline(always)]
pub fn is_initialized() -> bool {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        cr0_ts_is_set() // ← état réel, per-CPU, non falsifiable
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}
```

**Option B :** Conserver le flag mais le rendre per-CPU via `percpu.rs`.

```rust
// Dans percpu.rs : ajouter un champ à PerCpuData
pub fpu_lazy_init: bool,  // GS offset non contraint par l'ASM

// Dans lazy.rs :
pub fn init() {
    unsafe { cr0_set_ts(); }
    // Marquer CE CPU comme initialisé
    crate::arch::x86_64::smp::percpu::current_per_cpu_mut().fpu_lazy_init = true;
}

pub fn is_initialized() -> bool {
    crate::arch::x86_64::smp::percpu::current_per_cpu().fpu_lazy_init
}
```

**Recommandation :** Option A, plus simple et infalsifiable.

---

## DEF-15 — `SYSRETQ` Non Protégé : RSP Sans Vérification de Canonicité (P1)

### Localisation

`kernel/src/arch/x86_64/syscall.rs` — stub ASM `syscall_entry_asm`

### Description du défaut

```asm
// ── 10. Restaurer RSP userspace depuis gs:[0x08] ──────────
"mov   rsp, qword ptr gs:[0x08]",  // ← RSP userspace non validé
// ── 11. Restaurer GS userspace ───────────────────────────
"swapgs",
// ── 12. Retour en mode 64-bit Ring 3 ─────────────────────
"sysretq",
```

**Vulnérabilité connue — SYSRET bug Intel (CVE-2012-0217) :**

Si `RSP` contient une adresse non-canonique lors de `SYSRETQ`, sur les processeurs Intel, le processeur génère un `#GP` avec CPL=0 (Ring 0). Cela signifie que :
1. L'exception `#GP` est traitée en Ring 0
2. Mais avec la pile pointant vers une adresse contrôlée par l'utilisateur (RSP non-canonique)
3. Un attaquant peut provoquer une **exécution de code kernel arbitraire** en plaçant du code à l'adresse que RSP pointe lors du `#GP`

Cette vulnérabilité est classique depuis Linux 2012 (commit `d98f73b`).

**Conditions d'exploitation :** un processus Ring 3 peut invoquer un syscall avec `RSP` sauvegardé à une valeur non-canonique (ex. `0x8000_0000_0000_0000`). Le kernel sauvegarde ce RSP dans `gs:[0x08]` sans vérification, puis le restaure avant `SYSRETQ`.

### Correction

Valider la canonicité de `RSP` avant `SYSRETQ`. Sur x86_64 sans 5-level paging, les adresses canoniques valides pour userspace sont `0x0000_0000_0000_0000` à `0x0000_7FFF_FFFF_FFFF`.

```asm
// Ajouter AVANT swapgs + sysretq :
// ── 10. Restaurer RSP userspace depuis gs:[0x08] ──────────────────────────
"mov   rsp, qword ptr gs:[0x08]",
// ── 10b. Vérifier la canonicité de RSP (protection SYSRET CVE-2012-0217) ──
// RSP userspace valide : bits 63:47 = 0 (adresse userspace canonique)
// Si non-canonique → #GP via SYSRETQ = vuln Intel → forcer IRETQ à la place
"mov   rcx, rsp",
"sar   rcx, 47",           // propager le bit 47 dans tous les bits supérieurs
"test  rcx, rcx",          // si 0 → canonique (userspace) ; si non-0 → non-canonique
"jnz   .Lsysret_iret_fallback",  // non-canonique → fallback IRETQ
// ── 11. Restaurer GS userspace ───────────────────────────────────────────
"swapgs",
// ── 12. Retour en mode 64-bit Ring 3 ─────────────────────────────────────
"sysretq",
// ── Fallback IRETQ (RSP non-canonique) ───────────────────────────────────
".Lsysret_iret_fallback:",
// Construire un frame IRETQ minimal sur la pile kernel
// (pile kernel encore valide car rsp pointe kernel stack via gs:[0x00] shift)
// SAFETY : frame kernel, pas de leak possible
"swapgs",
"push   $0x23",            // SS ring3
"push   rsp",              // RSP (non-canonique — on laisse #GP au processus)
"push   r11",              // RFLAGS
"push   $0x2B",            // CS ring3 (USER_CS64)
"push   rcx",              // RIP (rcx = retour userspace depuis SYSCALL)
"iretq",
```

**Note :** Cette correction doit s'appliquer aussi à `syscall_cstar_noop` pour le mode 32-bit compat.

**Alternative plus simple** (recommandée en production) : utiliser `IRETQ` systématiquement pour les threads dont le RSP n'est pas certifié canonique, ou valider `user_rsp` dans `syscall_rust_handler` avant de revenir.

---

## DEF-16 — Page Fault Handler : `find_vma()` Retourne Toujours `None` (P1)

### Localisation

`kernel/src/arch/x86_64/exceptions.rs` — `do_page_fault()`  
`kernel/src/memory/virtual/fault/mod.rs` — `FaultContext::find_vma()`

### Description du défaut

```rust
// exceptions.rs
let ctx = FaultContext::new(fault_addr, cause, from_kernel);
// ← FaultContext::with_vma() JAMAIS appelé → vma_ptr reste null

let result = handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC);
```

```rust
// fault/mod.rs
pub fn find_vma(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
    if self.vma_ptr.is_null() {
        return None;  // ← toujours None car vma_ptr jamais fourni
    }
    // ...
}
```

```rust
// fault/handler.rs
let vma = match ctx.find_vma(ctx.fault_addr) {
    Some(v) => v,
    None => {                                     // ← TOUJOURS ce chemin
        FAULT_STATS.not_mapped.fetch_add(1, ...);
        return FaultResult::Segfault { ... };     // ← tout page fault = Segfault
    }
};
```

**Conséquence catastrophique :** tout page fault utilisateur — qu'il soit demand paging, CoW, ou swap-in — retourne `FaultResult::Segfault`. Le sous-système de demand paging est complètement inactif. Tout accès à une page non encore allouée tue le processus.

C'est le défaut le plus impactant sur le plan fonctionnel : **l'OS ne peut pas allouer de mémoire à la demande pour les processus utilisateur**.

### Cause

La lookup de VMA requiert l'espace d'adressage du processus courant — une dépendance vers `process/`. Or le commentaire dans `exceptions.rs` dit :

```rust
/// ## Intégration process/ (RÈGLE DOC1)
/// Quand process/ sera intégré, l'allocateur utilisera l'espace d'adressage
/// du processus courant. Pour l'instant, `KernelFaultAllocator` est utilisé.
```

Le code est intentionnellement incomplet mais aucune route de contournement n'existe — tout fault userspace aboutit à Segfault.

### Correction

Implémenter la lookup VMA dans le fault handler en utilisant la TCB courante pour accéder à l'espace d'adressage du processus :

```rust
// Dans exceptions.rs — do_page_fault() :

// Construire le FaultContext avec VMA lookup
let fault_addr = VirtAddr::new(fault_addr_raw);
let from_kernel = frame.from_kernel();

// Récupérer l'espace d'adressage du processus courant
let result = if !from_kernel {
    // Chemin userspace : lookup dans l'address space du process courant
    let tcb_ptr = unsafe {
        crate::arch::x86_64::smp::percpu::read_current_tcb() as *const crate::scheduler::core::task::ThreadControlBlock
    };
    
    if tcb_ptr.is_null() {
        // Boot ou idle : traiter comme kernel fault
        let ctx = FaultContext::new(fault_addr, cause, true);
        handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
    } else {
        // Lookup VMA via le registre process lié au TCB
        let pid = unsafe { (*tcb_ptr).pid };
        
        match crate::process::PROCESS_REGISTRY.find_by_pid(pid) {
            Some(process) => {
                let vma_ptr = process.address_space.find_vma(fault_addr)
                    .map(|v| v as *const _)
                    .unwrap_or(core::ptr::null());
                    
                let ctx = FaultContext::new(fault_addr, cause, false)
                    .with_vma(vma_ptr);
                    
                // Utiliser l'allocateur du processus (mappe dans son espace)
                handle_page_fault(&ctx, &process.fault_allocator())
            }
            None => {
                // Processus introuvable — Segfault
                FaultResult::Segfault { addr: fault_addr }
            }
        }
    }
} else {
    let ctx = FaultContext::new(fault_addr, cause, true);
    handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
};
```

**Note :** Cette correction requiert que `process::PROCESS_REGISTRY` soit accessible depuis `arch/`. La dépendance arch → process est acceptable via une interface de trait (inversion de dépendance) pour respecter les règles de layering DOC1.

**Interface de trait recommandée :**

```rust
// Dans memory/virtual/fault/mod.rs — à ajouter :
/// Interface d'accès à l'espace d'adressage du processus courant.
/// Enregistrée par process/ après son initialisation.
pub trait CurrentAddressSpace: Sync {
    fn find_vma(&self, addr: VirtAddr) -> Option<*const VmaDescriptor>;
}

static CURRENT_AS_PROVIDER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
```

---

## DEF-17 — Absence de LFENCE Après Lecture du Magic Handshake AP (P1)

### Localisation

`kernel/src/arch/x86_64/smp/init.rs` — boucle de polling dans `boot_ap()`

### Description du défaut

```rust
fn boot_ap(dest_apic_id: u8) {
    write_trampoline_u32(HANDSHAKE_OFFSET, 0);
    ipi::send_init_ipi(dest_apic_id);
    tsc::tsc_delay_ms(INIT_IPI_DELAY_MS);
    ipi::send_startup_ipi(dest_apic_id, TRAMPOLINE_PAGE);
    // ...
    loop {
        let sig = read_trampoline_u32(HANDSHAKE_OFFSET);  // ← volatile read
        if sig == AP_ALIVE_MAGIC {
            break;  // ← continue sans barrière
        }
        // ...
    }
}
// Après boot_ap() : le BSP accède aux données initialisées par l'AP
// (structures per-CPU, run queues, etc.) sans synchronisation explicite
```

```rust
fn read_trampoline_u32(offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((TRAMPOLINE_PHYS + offset) as *const u32) }
}
```

**Problème :** Sur Intel, les loads peuvent être réordonnés spéculativement (Load-Load reordering dans certains scénarios). Plus précisément, la spécification x86 garantit que les loads sont totalement ordonnés relativement aux autres loads du même CPU — mais le `volatile` ne garantit pas l'ordre vis-à-vis des stores de l'AP sur d'autres variables.

Sans `LFENCE` (ou équivalent `Acquire` sur l'AtomicU64 canonique), le BSP pourrait lire les données initialisées par l'AP avant d'observer la valeur finale dans `HANDSHAKE_OFFSET`.

**Impact :** Course possible entre le BSP lisant les structures per-CPU de l'AP et l'AP les initialisant. En pratique, le délai `tsc_delay_ms` entre les étapes offre une fenêtre de sécurité, mais ce n'est pas une garantie formelle.

### Correction

```rust
fn boot_ap(dest_apic_id: u8) {
    // ...
    loop {
        let sig = read_trampoline_u32(HANDSHAKE_OFFSET);
        if sig == AP_ALIVE_MAGIC {
            // CORRECTIF DEF-17 : barrière mémoire lecture avant d'accéder
            // aux structures initialisées par l'AP (per-CPU data, run queue, etc.)
            // LFENCE garantit que tous les loads suivants observent les stores
            // de l'AP qui ont précédé l'écriture de AP_ALIVE_MAGIC.
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            break;
        }
        if tsc::read_tsc() > deadline { break; }
        core::hint::spin_loop();
    }
}
```

Alternativement, utiliser un `AtomicU32` pour le handshake plutôt qu'un `read_volatile` sur la zone trampoline :

```rust
// Remplacer HANDSHAKE_OFFSET volatile reads par un AtomicU32 dans la zone trampoline
// en utilisant un AtomicU32 statique partagé par AP ID :
static AP_HANDSHAKE: [AtomicU32; MAX_CPUS] = { ... };
// L'AP écrit : AP_HANDSHAKE[cpu_id].store(AP_ALIVE_MAGIC, Ordering::Release)
// Le BSP lit : AP_HANDSHAKE[cpu_id].load(Ordering::Acquire)
```

---

## DEF-18 — `AtomicRefCount::inc()` : `Relaxed` Sans Barrière Avant Accès au Frame (P1)

### Localisation

`kernel/src/memory/physical/frame/ref_count.rs` — méthode `inc()`

### Description du défaut

```rust
pub fn inc(&self) -> u32 {
    let prev = self.0.fetch_add(1, Ordering::Relaxed);  // ← Relaxed !
    debug_assert_ne!(prev, u32::MAX, "AtomicRefCount overflow");
    prev + 1
}
```

**Scénario problématique :** Thread A incrémente le refcount du Frame X via `inc(Relaxed)`, puis accède au Frame X (le lit/écrit). Thread B décrémente le refcount et libère le frame.

```
Thread A                            Thread B
inc(Relaxed) → refcount = 2         
                                    dec(AcqRel) → refcount = 1
                                    dec(AcqRel) → refcount = 0 → free(Frame X)
read Frame X  ← potentiellement
               après la libération
               (aucune barrière A)
```

`Relaxed` ne garantit aucune synchronisation happens-before. Thread A peut lire Frame X après que Thread B l'a libéré et alloué à un autre usage.

La correction correcte est d'utiliser `AcqRel` pour `inc()` afin de synchroniser avec tout `dec()` précédent qui aurait pu libérer le frame.

### Correction

```rust
pub fn inc(&self) -> u32 {
    // Acquire garantit que les stores précédant le release du dernier dec()
    // (qui avait amené le refcount à 0 puis il a été réalloué) sont visibles
    // avant notre utilisation du frame.
    // Release garantit que nos stores dans le frame sont visibles aux futurs
    // dec() et inc() en Acquire.
    let prev = self.0.fetch_add(1, Ordering::AcqRel);
    debug_assert_ne!(prev, u32::MAX, "AtomicRefCount overflow");
    prev + 1
}
```

**Note sur la performance :** `AcqRel` sur x86_64 est implémenté par `lock xadd` qui est déjà une barrière totale. Le coût est identique à `Relaxed` sur cette architecture. Sur ARM/RISC-V, il y a un coût réel mais la correction est nécessaire pour l'exactitude.

---

## DEF-19 — `nr_running_usize()` Sans Synchronisation Dans le Hot Path Tick (P1)

### Localisation

`kernel/src/scheduler/timer/tick.rs` — `scheduler_tick()`  
`kernel/src/scheduler/core/runqueue.rs` — `nr_running_usize()`

### Description du défaut

```rust
// tick.rs
let rq = runqueue::run_queue(CpuId(cpu_id));
let nr = rq.nr_running_usize();  // ← lecture non-atomique ?
```

```rust
// runqueue.rs (hypothèse de lecture depuis le code réel)
pub fn nr_running_usize(&self) -> usize {
    // Potentiellement : lecture directe d'un champ non-atomique
    self.nr_running as usize
}
```

Si `nr_running` est un entier ordinaire protégé par le spinlock de la run queue, mais que `scheduler_tick()` lit `nr_running_usize()` **sans** tenir le lock (car le tick handler ne doit pas bloquer), il y a une course de données.

Ce `nr` est utilisé pour calculer le quantum CFS :

```rust
let nr = rq.nr_running_usize();
// ... (calcul du timeslice basé sur nr)
let timeslice = timeslice_for(tcb, nr, total_weight);
```

Un `nr` incorrect produit un quantum erroné : trop grand (une seule tâche semble présente) ou nul (division par zéro protégée par `max(1)`).

### Correction

Utiliser un compteur atomique séparé pour `nr_running` lisible sans verrou :

```rust
// Dans PerCpuRunQueue :
pub struct PerCpuRunQueue {
    // ...
    /// Nombre de threads prêts (atomique pour lecture depuis tick handler sans lock)
    pub nr_running_atomic: AtomicU32,
    // ...
}

// Mise à jour lors de enqueue/dequeue :
pub fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) {
    // ... logique enqueue ...
    self.nr_running_atomic.fetch_add(1, Ordering::Relaxed);
}
pub fn dequeue(&mut self) -> Option<...> {
    // ... logique dequeue ...
    self.nr_running_atomic.fetch_sub(1, Ordering::Relaxed);
}

// Lecture lock-free depuis le tick :
pub fn nr_running_usize(&self) -> usize {
    self.nr_running_atomic.load(Ordering::Relaxed) as usize
}
```

Le `Relaxed` est suffisant ici car `nr_running` est une estimation pour le calcul de quantum, pas une valeur critique de correctness.

---

## DEF-20 — `kpti_switch_to_user()` : Panic Sur CPU Sans Context Switch (P2)

### Localisation

`kernel/src/arch/x86_64/spectre/kpti.rs` — `kpti_switch_to_user()`

### Description du défaut

```rust
pub unsafe fn kpti_switch_to_user() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) { return; }
    let slot = current_cpu_slot();
    let cr3_user = slot.user.load(Ordering::Acquire);
    if cr3_user == 0 {
        panic!("KPTI actif mais aucun CR3 user n'est publie pour ce CPU");
    }
    // ...
}
```

**Scénario :** KPTI est activé. Un AP vient de démarrer (step 9 dans `ap_entry()`) mais n'a pas encore exécuté un context switch qui aurait appelé `set_current_cr3()`. Si une interruption sur cet AP tente de retourner vers Ring 3 via `exception_return_to_user()` → `kpti_switch_to_user()`, le slot `cr3_user` est 0 → **panic kernel**.

Cela peut arriver si un IRQ arrive entre `STI` et le premier context switch scheduler.

### Correction

Au lieu de paniquer, retourner sans switch si le slot est non-initialisé et logger un avertissement :

```rust
pub unsafe fn kpti_switch_to_user() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) { return; }
    let slot = current_cpu_slot();
    let cr3_user = slot.user.load(Ordering::Acquire);
    if cr3_user == 0 {
        // CPU non encore initialisé pour KPTI (premier context switch non effectué).
        // Comportement sécuritaire : ne pas switcher CR3 — rester sur le CR3 kernel.
        // Tout retour vers Ring 3 sans KPTI user CR3 est une fuite potential.
        // En pratique, sur un AP au boot, aucun thread userspace ne tourne encore.
        // Logger et continuer (pas de panic — l'AP est en cours d'init).
        #[cfg(debug_assertions)]
        crate::arch::x86_64::vga_early::print_str("KPTI: cr3_user=0 sur AP en init\n");
        return; // Sécuritaire : aucun thread user sur cet AP à ce stade
    }
    // ... switch normal ...
}
```

**Variante plus stricte :** Initialiser `cr3_user` avec la PML4 kernel (même valeur que `cr3_kernel`) lors de `init_percpu_for_ap()` pour garantir qu'un switch CR3 est toujours valide :

```rust
// Dans percpu.rs — init_percpu_for_ap() :
// Pré-initialiser le slot KPTI avec la PML4 kernel courante (CR3 courant)
// jusqu'au premier vrai context switch qui insérera la PML4 user correcte.
let kernel_cr3 = crate::arch::x86_64::read_cr3();
crate::arch::x86_64::spectre::kpti::set_current_cr3(kernel_cr3, kernel_cr3);
// Note : user=kernel est une approximation sécurisée (pas de mapping user exposé)
// mais évite le panic. Le premier context switch corrigera les deux valeurs.
```

---

## DEF-21 — VT-d Root Table : `write_bytes` Sans `compiler_fence` Avant SRTP (P2)

### Localisation

`kernel/src/memory/dma/iommu/intel_vtd.rs` — méthode `RootTable::zero()`

### Description du défaut

```rust
impl RootTable {
    pub fn zero(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 256);
            // ↑ Ne génère pas de barrière mémoire implicite
        }
        // Après ce retour, le code IOMMU écrit RTADDR + GCMD.SRTP pour activer la Root Table
        // SANS barrière entre write_bytes et GCMD.SRTP
    }
}
```

**Problème :** Le compilateur Rust peut théoriquement réordonner `write_bytes` après l'écriture MMIO du registre `GCMD.SRTP`. En pratique, les écriture MMIO via des pointeurs `volatile` constituent une barrière côté compilateur, mais `write_bytes` sur une zone normale (pas `volatile`) peut être réordonné.

Le CPU Intel x86_64 preserve l'ordre des stores, mais le compilateur peut réordonner les stores non-atomiques vis-à-vis des stores MMIO (qui passent par des intrinsics Rust différents).

### Correction

Ajouter un `compiler_fence(SeqCst)` après `zero()` et avant l'écriture de `RTADDR` :

```rust
impl RootTable {
    pub fn zero(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 256);
        }
        // Barrière compilateur : garantit que write_bytes est émis AVANT
        // tout code MMIO qui suit (RTADDR write, GCMD.SRTP).
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}
```

Ajouter également un `mfence` avant l'écriture de `GCMD.SRTP` dans le code d'initialisation IOMMU pour garantir l'ordre CPU :

```rust
// Dans le code d'init VT-d qui suit zero() :
// crate::arch::x86_64::memory_barrier(); // mfence
mmio_write32(vtd_base + vtd_regs::RTADDR, root_table_phys_lo);
mmio_write32(vtd_base + vtd_regs::RTADDR + 4, root_table_phys_hi);
core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
mmio_write32(vtd_base + vtd_regs::GCMD, gcmd_bits::SRTP);
```

---

## DEF-22 — `ELAPSED_NS` Non Réinitialisé au Démarrage des APs (P2)

### Localisation

`kernel/src/scheduler/timer/tick.rs` — tableaux `ELAPSED_NS` et `LAST_TCB_PTR`

### Description du défaut

`ELAPSED_NS` et `LAST_TCB_PTR` sont des statiques initialisées à 0. Lors du premier tick sur un AP nouvellement démarré :

```rust
let cpu_idx = (cpu_id as usize).min(255);
let current_ptr = current as usize;

// BUG-FIX R : remettre ELAPSED_NS à zéro quand un nouveau thread est détecté.
if LAST_TCB_PTR[cpu_idx].load(Ordering::Relaxed) != current_ptr {
    ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
    LAST_TCB_PTR[cpu_idx].store(current_ptr, Ordering::Relaxed);
}
let elapsed = ELAPSED_NS[cpu_idx].fetch_add(TICK_NS, Ordering::Relaxed) + TICK_NS;
```

Lors du démarrage, `LAST_TCB_PTR[cpu_idx] == 0` et `current_ptr == adresse_du_boot_idle_tcb`. La condition est vraie → `ELAPSED_NS` est remis à 0 → correct.

**Mais** : si un AP redémarre après hotplug offline/online, `LAST_TCB_PTR[cpu_idx]` peut contenir l'adresse de l'ancien boot idle TCB de la session précédente. Si le nouvel idle TCB a la **même adresse** (réutilisation statique), la condition est fausse → `ELAPSED_NS` n'est pas remis à 0 → le premier quantum est calculé avec une valeur résiduelle potentiellement grande (jusqu'à saturation).

### Correction

Réinitialiser explicitement les slots per-CPU lors de l'initialisation du tick sur un AP :

```rust
// Ajouter dans tick.rs :

/// Réinitialise les compteurs de tick pour le CPU `cpu_id`.
/// Appelé lors du démarrage ou redémarrage d'un AP.
pub fn reset_tick_counters(cpu_id: usize) {
    let idx = cpu_id.min(MAX_CPUS - 1);
    ELAPSED_NS[idx].store(0, Ordering::Relaxed);
    LAST_TCB_PTR[idx].store(0, Ordering::Relaxed);
}
```

Appeler `reset_tick_counters(cpu_id)` dans `scheduler::init_ap()` :

```rust
// scheduler/mod.rs
pub unsafe fn init_ap(cpu_id: u32) {
    self::fpu::lazy::init();
    // Réinitialiser les compteurs tick pour cet AP (hotplug safe)
    self::timer::tick::reset_tick_counters(cpu_id as usize);
}
```

---

## Analyse de Cohérence TLA+ — Compléments

### `Memory.tla` vs `percpu.rs` (ONLINE_CPU_COUNT)

Le modèle TLA+ `Memory.tla` définit `BSP_SetSecurityReady` comme nécessitant `iommu_init = 1` avant d'écrire `SECURITY_READY`. Dans `percpu.rs`, `ONLINE_CPU_COUNT.fetch_add(Release)` est la barrière de synchronisation entre BSP et APs pour signaler leur présence.

**Incohérence :** Le modèle TLA+ modélise `AP_SyncIommu(c)` comme un `ReadAcquire(c, "iommu_init")`, mais dans le code, les APs attendent `SECURITY_READY` (pas `iommu_init`). La structure de synchronisation code ≠ modèle pour l'IOMMU.

**Impact :** Sans conséquence directe si `SECURITY_READY` est positionné après l'init IOMMU. Mais le modèle devrait être mis à jour pour refléter que les APs attendent `SECURITY_READY` et non `iommu_init` directement.

### `ContextSwitch.tla` — Étapes Missing

Le modèle TLA+ dans `ContextSwitch.tla` modélise 5 étapes (0 → 5). Le code `switch.rs` en implémente 8 (avec PKRS, CET, FS/GS). Les étapes TLA+ `Step3_4_Internal` correspondent à une plage floue « 3..4 » ce qui ne modélise pas :
- La sauvegarde/restauration de PKRS
- La sauvegarde/restauration de CET SSP
- La mise à jour CURRENT_THREAD_PER_CPU

Le modèle TLA+ devrait être étendu pour ces étapes ou un commentaire explicite devrait documenter les étapes « hors scope formel ».

---

## Récapitulatif Global Parties 1 & 2

### Correctifs P0 (bloquants — doivent être appliqués en priorité absolue)

| ID | Action immédiate |
|----|-----------------|
| DEF-01 | Supprimer `percpu::preempt_disable/enable`, exposer `is_preempt_disabled()` canonical |
| DEF-13 | Ajouter `crate::scheduler::init_ap(cpu_id)` dans `ap_entry()` |
| DEF-14 | Remplacer `FPU_LAZY_INITIALIZED` global par lecture réelle de `CR0.TS` |

### Correctifs P1 (majeurs — à appliquer avant toute mise en service)

| ID | Action |
|----|--------|
| DEF-02 | Supprimer stacks IST inutilisées (économie 8-12 MiB BSS) |
| DEF-06 | `build_user_shadow_pml4` : ne pas copier PML4[511] entier |
| DEF-07 | Réordonner `set_current_tcb` avant `update_rsp0` dans `context_switch()` |
| DEF-08 | Aligner ordre CR0.TS avec le modèle TLA+ (avant l'ASM switch) |
| DEF-15 | Valider canonicité RSP avant `SYSRETQ` (CVE-2012-0217) |
| DEF-16 | Implémenter le lookup VMA dans le page fault handler |
| DEF-17 | Ajouter `fence(Acquire)` après détection `AP_ALIVE_MAGIC` |
| DEF-18 | `AtomicRefCount::inc()` : utiliser `AcqRel` au lieu de `Relaxed` |
| DEF-19 | `nr_running_usize()` : utiliser un compteur atomique séparé |
| DEF-04 | `should_enable_kpti()` : lire CPUID + IA32_ARCH_CAPABILITIES |

### Correctifs P2 (qualité)

| ID | Action |
|----|--------|
| DEF-03 | Corriger la documentation de `extern "C" fn context_switch_asm` |
| DEF-05 | Remplacer `[; 256]` par `[; MAX_CPUS]` dans `tick.rs` |
| DEF-09 | Clarifier le layout `_cold_reserve`, ajouter assertions `affinity_hi` |
| DEF-10 | `AtomicRefCount::dec()` : utiliser CAS loop |
| DEF-11 | `ensure_boot_idle_tcb()` : utiliser `compare_exchange` |
| DEF-12 | Supprimer `memory_barrier()` redondant avant `fetch_add(Release)` |
| DEF-20 | `kpti_switch_to_user()` : ne pas paniquer si slot non-initialisé |
| DEF-21 | `RootTable::zero()` : ajouter `compiler_fence(SeqCst)` |
| DEF-22 | Ajouter `reset_tick_counters()` dans `scheduler::init_ap()` |

---

## Ordre d'Application Recommandé

```
1. DEF-13 (CR0.TS APs)           — fonctionnel de base
2. DEF-14 (FPU_LAZY flag)        — dépend de DEF-13
3. DEF-01 (double preempt count) — sécurité scheduler
4. DEF-16 (page fault VMA)       — fonctionnel processus
5. DEF-15 (SYSRETQ canonicité)   — sécurité CVE
6. DEF-06 (KPTI PML4[511])       — sécurité KPTI
7. DEF-07 (TCB/RSP0 ordering)    — correctness switch
8. DEF-08 (CR0.TS ordering)      — cohérence TLA+
9. DEF-18 (refcount ordering)    — correctness mémoire
10. DEF-02, DEF-04, DEF-17...    — les P1 restants
11. Tous les P2                   — derniers
```

---

*Document Partie 2 — claude-beta — 2026-05-03*  
*Total défauts identifiés : 22 (12 Partie 1 + 10 Partie 2)*  
*Criticité : 3 P0, 12 P1, 7 P2*
