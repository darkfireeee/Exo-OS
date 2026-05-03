# ExoOS — Corrections Modules `arch` / `memory` / `scheduler`
> **Auteur : claude-gamma**  
> **Date : 2026-05-03**  
> **Dépôt analysé : `https://github.com/darkfireeee/Exo-OS.git` (branche `main`)**  
> **Périmètre : `kernel/src/arch/`, `kernel/src/memory/`, `kernel/src/scheduler/`, `docs/Exo-OS-TLA+/`**

---

## Sommaire des défauts

| ID | Module | Fichier | Sévérité | Catégorie |
|----|--------|---------|----------|-----------|
| [ARCH-01](#arch-01) | arch | `paging.rs` | **P0 CRITIQUE** | Bogue fonctionnel — userspace cassé |
| [ARCH-02](#arch-02) | arch | `gdt.rs` | **P1 MAJEUR** | Validation manquante — SYSCALL silencieux |
| [ARCH-03](#arch-03) | arch | `switch.rs` | **P1 MAJEUR** | Commentaire FFI erroné — FPU state |
| [ARCH-04](#arch-04) | arch | `paging.rs` | P2 mineur | Variants enum morts |
| [ARCH-05](#arch-05) | arch | `idt.rs` | P2 mineur | Constante redondante |
| [SCHED-01](#sched-01) | scheduler | `switch.rs` | **P0 CRITIQUE** | `switch_count` jamais incrémenté |
| [SCHED-02](#sched-02) | scheduler | `switch.rs` | **P1 MAJEUR** | `run_time_acc` jamais mis à jour |
| [SCHED-03](#sched-03) | scheduler | `runqueue.rs` | **P1 MAJEUR** | Thread silencieusement perdu quand RQ pleine |
| [SCHED-04](#sched-04) | scheduler | `runqueue.rs` | **P1 MAJEUR** | Statistiques DEADLINE mal comptabilisées |
| [SCHED-05](#sched-05) | scheduler | `switch.rs` | P2 mineur | Code mort — signal check ignoré |
| [SCHED-06](#sched-06) | scheduler | `runqueue.rs` | P2 mineur | `min_vruntime` Relaxed store sans mémoire de cohérence |
| [SCHED-07](#sched-07) | scheduler | `SCHEDULER_CORE.md` | P2 doc | TCB décrit 128 B / `ThreadId u32` au lieu de 256 B / `u64` |
| [MEM-01](#mem-01) | memory | `paging.rs` | **P0 CRITIQUE** | `PTE_USER` absent dans les tables intermédiaires |
| [MEM-02](#mem-02) | memory | `mod.rs` | P2 mineur | Commentaire API incorrect sur `alloc_zeroed_page` |
| [TLA-01](#tla-01) | TLA+ | `ContextSwitch.tla` | **P1 MAJEUR** | `Step2_SetLazyBit` diverge du code Rust |

---

## Légende de sévérité

- **P0 CRITIQUE** : corruption silencieuse de données ou panique irrecupérable en production
- **P1 MAJEUR** : comportement incorrect observable, régression fonctionnelle, sécurité compromise
- **P2 mineur** : code mort, commentaire trompeur, métrique brisée, dette technique

---

---

# MODULE ARCH

---

## ARCH-01

**Fichier :** `kernel/src/arch/x86_64/paging.rs`  
**Sévérité :** P0 CRITIQUE — tous les mappages userspace génèrent un `#PF`  
**Règle violée :** x86_64 SDM Vol. 3 §4.6 — Table 4-2 : *"If the U/S flag of any paging-structure entry is 0, supervisor-mode accesses are allowed but user-mode accesses are not."*

### Analyse

La fonction `get_or_create_subtable()` crée les entrées de tables intermédiaires (PML4E, PDPTE, PDE) avec uniquement :

```rust
// DÉFAUT — extrait actuel de paging.rs ligne ~260
*entry = PageTableEntry::new(phys, PTE_PRESENT | PTE_WRITABLE);
```

Le bit `PTE_USER (1 << 2)` est absent. Sur x86_64, si **l'une quelconque** des entrées d'un chemin de traduction manque du bit U/S, le processeur déclenche un `#PF` en Ring 3 quelle que soit la valeur de l'entrée feuille (PT). Résultat : **tous les appels à `map_4k_page()` qui spécifient `PAGE_FLAGS_USER_*` en flags feuille produisent un mapping apparemment réussi mais toujours inaccessible depuis l'espace utilisateur.**

### Correctif

```rust
// CORRECTION — kernel/src/arch/x86_64/paging.rs
// Fonction get_or_create_subtable()

fn get_or_create_subtable<'a>(
    parent: &'a mut PageTable,
    idx: usize,
    alloc_page: &impl Fn() -> Option<u64>,
    // NOUVEAU PARAMÈTRE : indique si ce chemin doit être user-accessible
    is_user_mapping: bool,
) -> Result<&'a mut PageTable, PageTableError> {
    let entry = parent.entry_mut(idx);
    if !entry.is_present() {
        let phys = alloc_page().ok_or(PageTableError::OutOfMemory)?;
        // FIX ARCH-01 : propager PTE_USER dans les tables intermédiaires
        // si le mappage final est destiné à l'espace utilisateur.
        // Sans ce bit sur chaque niveau, le hardware refuse l'accès Ring 3
        // indépendamment des flags de l'entrée feuille (SDM Vol.3 §4.6).
        let flags = if is_user_mapping {
            PTE_PRESENT | PTE_WRITABLE | PTE_USER
        } else {
            PTE_PRESENT | PTE_WRITABLE
        };
        *entry = PageTableEntry::new(phys, flags);

        // Initialiser la sous-table à zéro
        // SAFETY: phys est une frame fraîchement allouée — pas de contenu préalable
        unsafe {
            let ptr = phys as *mut PageTable;
            (*ptr).clear();
        }
    }
    // SAFETY: l'entrée est présente et pointe vers une PageTable valide
    Ok(unsafe { &mut *(entry.phys_addr() as *mut PageTable) })
}

/// Mappage brut : installe une entrée dans la hiérarchie existante
///
/// # Safety
/// - `pml4` doit pointer vers la PML4 active ou en cours de construction
/// - `phys_page` et les tables intermédiaires doivent être des frames valides
/// - L'appelant garantit l'absence de race sur les tables de pages
pub unsafe fn map_4k_page(
    pml4: *mut PageTable,
    virt_addr: u64,
    phys_addr: u64,
    flags: u64,
    alloc_page: impl Fn() -> Option<u64>,
) -> Result<(), PageTableError> {
    let idx = decompose_virt_addr(virt_addr);
    // FIX ARCH-01 : détecter si le mappage final est destiné à Ring 3.
    let is_user = (flags & PTE_USER) != 0;

    // SAFETY: `pml4` est un pointeur valide passé par l'appelant (unsafe fn).
    let pml4 = unsafe { &mut *pml4 };
    // FIX ARCH-01 : transmettre `is_user` à chaque niveau intermédiaire.
    let pdpt = get_or_create_subtable(pml4, idx.pml4_idx, &alloc_page, is_user)?;
    let pd   = get_or_create_subtable(pdpt, idx.pdpt_idx, &alloc_page, is_user)?;
    let pt   = get_or_create_subtable(pd,   idx.pd_idx,   &alloc_page, is_user)?;

    if pt.entry(idx.pt_idx).is_present() {
        return Err(PageTableError::AlreadyMapped);
    }
    *pt.entry_mut(idx.pt_idx) = PageTableEntry::new(phys_addr, flags);

    PAGE_MAP_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}
```

> **Impact** : sans ce correctif, aucun processus utilisateur ne peut accéder à ses segments code, données, pile — le noyau doit paniquer ou boucler sur des #PF à l'infini dès `exec()`.

---

## ARCH-02

**Fichier :** `kernel/src/arch/x86_64/gdt.rs`  
**Sévérité :** P1 MAJEUR — SYSCALL/SYSRET silencieusement cassé si layout GDT incorrect  

### Analyse

La fonction `validate_star_layout()` vérifie que les sélecteurs GDT satisfont les contraintes du MSR STAR (SYSCALL CS/SS, SYSRET CS/SS). Elle est définie mais **jamais appelée** dans `init_gdt_for_cpu()`. Un refactoring futur qui déplacerait un sélecteur GDT passerait inaperçu jusqu'au premier appel `SYSCALL` — qui retournerait vers le mauvais segment, produisant une GPF ou un comportement indéfini.

### Correctif

```rust
// CORRECTION — kernel/src/arch/x86_64/gdt.rs
// Dans init_gdt_for_cpu(), ajouter la vérification après le chargement du GDT.

pub unsafe fn init_gdt_for_cpu(cpu_id: usize, kernel_stack_top: u64) {
    assert!(cpu_id < MAX_CPUS, "GDT: cpu_id hors bornes");

    tss::init_tss_for_cpu(cpu_id, kernel_stack_top);

    let gdt = unsafe { &mut CPU_GDTS[cpu_id] };
    gdt.install(cpu_id);

    let gdtr = GdtRegister {
        limit: (core::mem::size_of::<Gdt>() - 1) as u16,
        base: gdt.entries.as_ptr() as u64,
    };

    unsafe {
        core::arch::asm!(
            "lgdt [{gdtr}]",
            gdtr = in(reg) &gdtr as *const GdtRegister,
            options(nostack, nomem)
        );
    }

    unsafe {
        core::arch::asm!(
            "push {kcs}",
            "lea  {tmp}, [rip + 1f]",
            "push {tmp}",
            "retfq",
            "1:",
            kcs = in(reg) GDT_KERNEL_CS as u64,
            tmp = out(reg) _,
            options(nostack)
        );

        core::arch::asm!(
            "mov ax, {kds}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "xor ax, ax",
            "mov fs, ax",
            "mov gs, ax",
            kds = const GDT_KERNEL_DS,
            out("ax") _,
            options(nostack, nomem)
        );
    }

    unsafe { tss::load_tss(GDT_TSS_SEL); }

    // FIX ARCH-02 : vérifier le layout STAR immédiatement après chargement
    // du GDT. Paniquer si les contraintes SYSCALL/SYSRET ne sont pas satisfaites.
    // Cette vérification est O(1) et ne s'exécute qu'une fois par CPU au boot.
    assert!(
        validate_star_layout(),
        "GDT cpu{}: layout STAR invalide — SYSCALL/SYSRET cassé. \
         Vérifier la disposition des sélecteurs KERNEL_CS/DS et USER_CS32/DS/CS64.",
        cpu_id
    );

    GDT_INITIALIZED.store(true, Ordering::Release);
}
```

---

## ARCH-03

**Fichier :** `kernel/src/arch/x86_64/sched_iface.rs` → `kernel/src/scheduler/core/switch.rs`  
**Sévérité :** P1 MAJEUR — commentaire FFI trompeur sur la sauvegarde FPU  

### Analyse

Le bloc `extern "C"` dans `switch.rs` documente `context_switch_asm` ainsi :

```rust
// DÉFAUT — switch.rs, bloc extern "C"
/// Sauvegarde les registres callee-saved (rbx, rbp, r12-r15) + MXCSR + x87 FCW
/// du thread `old`, puis switche CR3 si nécessaire (KPTI), puis restaure
/// le contexte du thread `new`.
fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
```

Mais `switch_asm.s` ne sauvegarde **pas** MXCSR ni x87 FCW — ce comportement a été supprimé en V7-C-02 (le noyau est compilé avec `-mmx,-sse` donc le compilateur ne génère jamais d'instructions SSE dans le kernel et la FPU est gérée exclusivement par `XSAVE/XRSTOR` dans `fpu/save_restore.rs`).

Un développeur qui lit ce commentaire et fait confiance à la déclaration FFI pourrait créer du code qui s'appuie sur la préservation de MXCSR/FCW par `context_switch_asm`, introduisant une corruption de l'état FPU silencieuse.

### Correctif

```rust
// CORRECTION — kernel/src/scheduler/core/switch.rs

extern "C" {
    /// Context switch ASM bas niveau (V7-C-02).
    ///
    /// Sauvegarde les **6 registres callee-saved ABI System V uniquement**
    /// (rbx, rbp, r12–r15) du thread `old`, puis restaure ceux du thread `new`.
    ///
    /// **MXCSR et x87 FCW ne sont PAS sauvegardés ici** — ils sont gérés
    /// exclusivement par `scheduler::fpu::save_restore::xsave_current()` /
    /// `xrstor_to()` via les instructions XSAVE/XRSTOR. Le noyau est compilé
    /// sans SSE (`-mmx,-sse,-sse2,+soft-float`), donc le compilateur ne génère
    /// jamais d'instructions FPU dans les chemins noyau.
    ///
    /// # Arguments (System V ABI)
    /// - `old_kernel_rsp` : `*mut u64` pointant vers `TCB::kstack_ptr` du thread sortant
    /// - `new_kernel_rsp` : valeur du `TCB::kstack_ptr` du thread entrant
    /// - `new_cr3`        : registre CR3 du thread entrant (0 = pas de switch CR3)
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}
```

---

## ARCH-04

**Fichier :** `kernel/src/arch/x86_64/paging.rs`  
**Sévérité :** P2 mineur — variants enum jamais retournés  

### Analyse

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableError {
    AlreadyMapped,
    OutOfMemory,
    InvalidAlignment,  // ← jamais retourné
    NotMapped,         // ← jamais retourné
    HugePageConflict,  // ← jamais retourné
}
```

`map_4k_page` retourne uniquement `AlreadyMapped` et `OutOfMemory`. `unmap_4k_page` et `translate_virt` retournent `Option<u64>` sans erreur. Les trois variants orphelins trompent tout `match` exhaustif sur ce type.

### Correctif

Option A — implémenter et retourner ces variants là où logique :

```rust
// CORRECTION — kernel/src/arch/x86_64/paging.rs
// Dans map_4k_page(), ajouter la vérification d'alignement :

pub unsafe fn map_4k_page(
    pml4: *mut PageTable,
    virt_addr: u64,
    phys_addr: u64,
    flags: u64,
    alloc_page: impl Fn() -> Option<u64>,
) -> Result<(), PageTableError> {
    // FIX ARCH-04 : vérifier l'alignement à 4 KiB des deux adresses
    if virt_addr & 0xFFF != 0 || phys_addr & 0xFFF != 0 {
        return Err(PageTableError::InvalidAlignment);
    }
    // ... suite inchangée ...
}

// Dans unmap_4k_page(), retourner Result<u64, PageTableError> :
pub unsafe fn unmap_4k_page(
    pml4: *mut PageTable,
    virt_addr: u64,
) -> Result<u64, PageTableError> {
    // FIX ARCH-04 : vérifier l'alignement
    if virt_addr & 0xFFF != 0 {
        return Err(PageTableError::InvalidAlignment);
    }
    // Si l'entrée feuille est absente, retourner NotMapped
    // (au lieu de None qui masquait la sémantique)
    // ... retourner Err(PageTableError::NotMapped) si non trouvé ...
    // ... retourner Ok(phys) si succès ...
}
```

Option B (immédiate, moins invasive) — supprimer les variants orphelins et documenter l'intention :

```rust
// Option B : épuration immédiate
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableError {
    AlreadyMapped,
    OutOfMemory,
    // FIX ARCH-04 : InvalidAlignment, NotMapped, HugePageConflict supprimés —
    // non implémentés. À réintroduire avec leur usage effectif.
}
```

---

## ARCH-05

**Fichier :** `kernel/src/arch/x86_64/idt.rs`  
**Sévérité :** P2 mineur — constante redondante non utilisée  

### Analyse

```rust
pub const IRQ_BASE: u8 = 32;          // utilisé dans init_idt()
pub const VEC_IRQ_TIMER: u8 = 0x20;  // == 32, non utilisé dans init_idt()
```

`init_idt()` enregistre le timer sur `IRQ_BASE`, non sur `VEC_IRQ_TIMER`. La constante `VEC_IRQ_TIMER` est définie mais jamais utilisée dans ce fichier — c'est du code mort qui peut induire en erreur sur le vecteur réel.

### Correctif

```rust
// CORRECTION — kernel/src/arch/x86_64/idt.rs

// Supprimer VEC_IRQ_TIMER comme constante séparée et la redéfinir
// comme alias de IRQ_BASE pour clarté :

/// Vecteur IRQ timer (APIC Local Timer) — identique à IRQ_BASE
pub const VEC_IRQ_TIMER: u8 = IRQ_BASE; // 0x20 = 32

// Et dans init_idt() utiliser systématiquement VEC_IRQ_TIMER pour
// le timer afin que les deux constantes soient cohérentes et vérifiées
// à la compilation :
idt.set_handler(
    VEC_IRQ_TIMER,    // FIX ARCH-05 : utiliser VEC_IRQ_TIMER, pas IRQ_BASE
    irq_timer_handler as *const () as u64,
    0,
    IdtEntryFlags::INTERRUPT_GATE,
);
```

---

# MODULE SCHEDULER

---

## SCHED-01

**Fichier :** `kernel/src/scheduler/core/switch.rs`  
**Sévérité :** P0 CRITIQUE — `switch_count` et `ctx_switch_count` toujours à 0  

### Analyse

`ThreadControlBlock` déclare `switch_count: u64` (offset [136]) et `PerCpuData` déclare `ctx_switch_count: u64`. Ces compteurs servent au monitoring de performance, aux outils de débogage (strace, perf), et au load balancer SMP (`migration.rs` utilise potentiellement ces métriques). La fonction `context_switch()` ne les incrémente **jamais** : les deux champs restent à 0 depuis le boot pour tous les threads sur tous les CPUs.

### Correctif

```rust
// CORRECTION — kernel/src/scheduler/core/switch.rs
// Dans la fonction context_switch(), après "Étape 5 : Post-switch côté `next`"

    // ── Étape 5 : Post-switch côté `next` ────────────────────────────────────
    next.set_state(TaskState::Running);

    unsafe {
        tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);
        percpu::set_kernel_rsp(next.kstack_ptr);
    }

    percpu::set_current_tcb(next as *mut ThreadControlBlock);

    unsafe { fpu::lazy::cr0_set_ts(); }
    next.set_fpu_loaded(false);

    // FIX SCHED-01 : incrémenter switch_count sur le thread entrant.
    // Utiliser wrapping_add pour éviter panic en overflow après ~1.8×10^19 switches.
    next.switch_count = next.switch_count.wrapping_add(1);

    // FIX SCHED-01 : incrémenter le compteur per-CPU de context switches.
    // SAFETY: percpu::current_cpu_data() renvoie le slot GS du CPU courant,
    //         toujours initialisé avant l'activation des IRQ.
    unsafe {
        let cpu_data = percpu::current_cpu_data_mut();
        cpu_data.ctx_switch_count = cpu_data.ctx_switch_count.wrapping_add(1);
        cpu_data.last_switch_tsc = crate::arch::x86_64::cpu::tsc::read_tsc();
    }

    core::sync::atomic::fence(Ordering::SeqCst);
    CURRENT_THREAD_PER_CPU[next.current_cpu().0 as usize]
        .store(next as *mut ThreadControlBlock as usize, Ordering::Release);

    // ... suite inchangée (restauration FS/GS) ...
```

---

## SCHED-02

**Fichier :** `kernel/src/scheduler/core/switch.rs`  
**Sévérité :** P1 MAJEUR — `run_time_acc` toujours à 0 — comptabilisation CPU impossible  

### Analyse

`ThreadControlBlock::run_time_acc: u64` (offset [128]) est destiné à accumuler le temps CPU total passé en état `Running` par ce thread. Ce champ est lu nulle part car il est toujours à 0. Le module `TaskStats` (séparé du TCB) dispose de `run_time_ns`, mais ce champ **TCB** est celui qui doit être mis à jour dans le chemin chaud du context switch.

### Correctif

Implémenter la mise à jour de `run_time_acc` au moment du context switch sortant, en exploitant le TSC :

```rust
// CORRECTION — kernel/src/scheduler/core/switch.rs
// Dans context_switch(), juste avant l'appel à context_switch_asm() (Étape 4)

    // ── Étape 3.5 : Comptabilisation temps d'exécution de prev ───────────────
    // FIX SCHED-02 : accumuler le temps réel sur CPU de `prev` avant le switch.
    // On lit le TSC ici (point de sortie). La valeur d'entrée est stockée dans
    // `percpu::last_switch_tsc` depuis le switch précédent.
    //
    // SAFETY: read_tsc() est une instruction non-faillible Ring 0.
    let now_tsc = unsafe { crate::arch::x86_64::cpu::tsc::read_tsc() };
    // SAFETY: current_cpu_data() renvoie le slot GS du CPU courant, initialisé.
    let last_tsc = unsafe { percpu::current_cpu_data().last_switch_tsc };
    if now_tsc > last_tsc {
        // Convertir delta TSC → nanosecondes approximatif (1 ns ≈ 1 cycle @ 1 GHz)
        // Pour une précision exacte, utiliser crate::arch::x86_64::time::tsc_to_ns().
        let delta_ns = crate::arch::x86_64::time::tsc_to_ns(now_tsc - last_tsc);
        prev.run_time_acc = prev.run_time_acc.saturating_add(delta_ns);
    }

    // ── Étape 4 : ASM context switch ─────────────────────────────────────────
    let new_cr3 = if prev.cr3_phys != next.cr3_phys {
        next.cr3_phys
    } else {
        0
    };
    context_switch_asm(&mut prev.kstack_ptr as *mut u64, next.kstack_ptr, new_cr3);
```

> **Note :** si `crate::arch::x86_64::time::tsc_to_ns()` n'existe pas encore, utiliser `now_tsc - last_tsc` directement en unités TSC, avec une note indiquant la conversion future.

---

## SCHED-03

**Fichier :** `kernel/src/scheduler/core/runqueue.rs`  
**Sévérité :** P1 MAJEUR — perte silencieuse de thread quand la CFS run queue est pleine  

### Analyse

```rust
// DÉFAUT — runqueue.rs, CfsRunQueue::enqueue()
fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) {
    if self.count >= MAX_TASKS_PER_CPU {
        // File pleine : le thread sera re-tenté au prochain tick.
        // En pratique impossible avec 512 slots per-CPU.
        return;   // ← RETOUR SILENCIEUX — le thread est PERDU
    }
    // ...
}
```

Le commentaire justifie le silence par "impossible en pratique" — mais :
1. `return` ne réessaie pas au prochain tick — personne n'appelle à nouveau `enqueue()`.
2. L'appelant (`PerCpuRunQueue::enqueue`) incrémente quand même `nr_running` puis ne sait pas que le thread a été perdu.
3. Le thread `Runnable` devient un zombie — jamais exécuté, jamais signalé.

### Correctif

```rust
// CORRECTION — kernel/src/scheduler/core/runqueue.rs
// CfsRunQueue::enqueue() doit retourner un résultat

/// Insère un thread CFS en maintenant le tri par vruntime.
/// Retourne `false` si la file est pleine (overflow défensif).
fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
    if self.count >= MAX_TASKS_PER_CPU {
        // FIX SCHED-03 : ne JAMAIS silencieusement perdre un thread.
        // Log kernel critique + retourner false pour que l'appelant
        // puisse prendre une décision (panic, migration, retry).
        //
        // En pratique, MAX_TASKS_PER_CPU = 512 et le système devrait
        // avoir appliqué un admission control bien avant d'atteindre
        // cette borne. Si on l'atteint c'est un bug de conception.
        //
        // SAFETY: tcb est un NonNull valide — lecture de tid safe.
        let tid = unsafe { tcb.as_ref().tid };
        crate::kernel_warn!(
            "CFS RQ CPU{}: OVERFLOW — thread TID={} perdu ! \
             MAX_TASKS_PER_CPU={} atteint. Vérifier admission control.",
            self.count, // cpu_id non disponible ici, utiliser count comme proxy
            tid,
            MAX_TASKS_PER_CPU
        );
        return false;
    }
    // ... insertion inchangée ...
    true
}

// Et dans PerCpuRunQueue::enqueue() :
pub fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) {
    let policy = unsafe { tcb.as_ref() }.policy;
    let enqueued = match policy {
        SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
            self.rt.enqueue(tcb)
        }
        SchedPolicy::Normal | SchedPolicy::Batch => {
            self.cfs.enqueue(tcb)  // FIX SCHED-03 : vérifier le retour
        }
        SchedPolicy::Deadline => {
            unsafe {
                crate::scheduler::timer::deadline_timer::dl_enqueue(self.cpu.0 as usize, tcb);
            }
            true
        }
        SchedPolicy::Idle => { return; }
    };

    // FIX SCHED-03 : n'incrémenter nr_running que si l'enfilage a réussi.
    if enqueued {
        let prev = self.stats.nr_running.fetch_add(1, Ordering::Relaxed);
        self.update_load_avg(prev as u64 + 1);
    } else {
        // Situation non récupérable en production — paniquer plutôt que corrompre.
        panic!(
            "PerCpuRunQueue CPU{}: enqueue échoué pour TID={}, politique={:?}. \
             Run queue pleine ou bug d'allocation.",
            self.cpu.0,
            unsafe { tcb.as_ref().tid },
            policy
        );
    }
}
```

---

## SCHED-04

**Fichier :** `kernel/src/scheduler/core/runqueue.rs`  
**Sévérité :** P1 MAJEUR — threads DEADLINE comptabilisés dans les stats CFS  

### Analyse

```rust
// DÉFAUT — runqueue.rs, PerCpuRunQueue::pick_next()
let dl_candidate =
    unsafe { crate::scheduler::timer::deadline_timer::dl_pick_next(self.cpu.0 as usize) };
if let Some(tcb) = dl_candidate {
    self.stats.picks_cfs.fetch_add(1, Ordering::Relaxed); // ← MAUVAIS COMPTEUR
    self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
    return Some(tcb);
}
```

Les threads `SCHED_DEADLINE` (EDF) sont comptés dans `picks_cfs`. Tout outil qui se base sur `picks_cfs` vs `picks_rt` pour calculer le ratio CFS/DEADLINE obtient des chiffres faux.

### Correctif

```rust
// CORRECTION — kernel/src/scheduler/core/runqueue.rs
// Ajouter un compteur dédié dans RunQueueStats

#[repr(C)]
pub struct RunQueueStats {
    pub picks_total:    AtomicU64,
    pub picks_rt:       AtomicU64,
    pub picks_cfs:      AtomicU64,
    pub picks_dl:       AtomicU64, // FIX SCHED-04 : compteur DEADLINE dédié
    pub picks_idle:     AtomicU64,
    pub nr_running:     AtomicU32,
    pub load_avg:       AtomicU64,
    pub last_balance_ns: AtomicU64,
}

impl RunQueueStats {
    const fn new() -> Self {
        Self {
            picks_total: AtomicU64::new(0),
            picks_rt:    AtomicU64::new(0),
            picks_cfs:   AtomicU64::new(0),
            picks_dl:    AtomicU64::new(0), // FIX SCHED-04
            picks_idle:  AtomicU64::new(0),
            nr_running:  AtomicU32::new(0),
            load_avg:    AtomicU64::new(0),
            last_balance_ns: AtomicU64::new(0),
        }
    }
}

// Et dans pick_next() :
if let Some(tcb) = dl_candidate {
    self.stats.picks_dl.fetch_add(1, Ordering::Relaxed); // FIX SCHED-04
    self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
    return Some(tcb);
}
```

---

## SCHED-05

**Fichier :** `kernel/src/scheduler/core/switch.rs`  
**Sévérité :** P2 mineur — code mort trompeur (`check_signal_pending` ignoré)  

### Analyse

```rust
// DÉFAUT — switch.rs, fin de context_switch()
// Vérifier signal pending (lecture pure, pas de livraison).
let _sig = check_signal_pending(next); // résultat ignoré ici — arch/ s'en occupe
```

L'appel `check_signal_pending(next)` lit un `AtomicU64` (5 cycles) mais le résultat `_sig` est immédiatement jeté. Le commentaire dit "arch/ s'en occupe" — effectivement, `arch/syscall.rs` et `arch/exceptions.rs` liront ce flag au retour userspace. Ce code ne produit aucun effet observable et induit en erreur : un lecteur pourrait croire que le signal est traité ici.

### Correctif

```rust
// CORRECTION — kernel/src/scheduler/core/switch.rs
// Supprimer l'appel mort et remplacer par un commentaire explicatif.

    // ── Post-switch : signaux ─────────────────────────────────────────────────
    // FIX SCHED-05 : L'appel check_signal_pending() ici était mort (résultat ignoré).
    // La livraison des signaux est gérée par arch/syscall.rs (retour SYSRET/IRETQ)
    // et arch/exceptions.rs (retour post-exception) — pas depuis le scheduler.
    // RÈGLE SWITCH-01 : scheduler/ lit UNIQUEMENT le flag, ne livre JAMAIS.
    // L'action appropriée est donc de ne rien faire ici.
```

---

## SCHED-06

**Fichier :** `kernel/src/scheduler/core/runqueue.rs`  
**Sévérité :** P2 mineur — `min_vruntime` update avec `Ordering::Relaxed` sans barrière  

### Analyse

```rust
// DÉFAUT — runqueue.rs, CfsRunQueue::dequeue_min()
// Intentionnel: min_vruntime est une borne approximative CFS.
self.min_vruntime.store(new_min, Ordering::Relaxed);
```

Le commentaire dit "intentionnel" et "approximatif". Cependant, `dequeue_min()` est toujours appelé avec la préemption désactivée (invariant `PerCpuRunQueue`). Dans ce contexte monothread-CPU, `Relaxed` est équivalent à `Release` pour la visibilité locale. Le problème est que d'autres CPUs lisant `min_vruntime` pour le load balancing (ex: `cfs_dequeue_for_migration`) peuvent voir une valeur stale arbitrairement longtemps.

### Correctif

```rust
// CORRECTION — kernel/src/scheduler/core/runqueue.rs
// Utiliser Release pour garantir la visibilité cross-CPU du min_vruntime
// après un dequeue (le load balancer lit ce champ via Acquire).

if self.count > 0 {
    let new_min = unsafe {
        self.tasks[0]
            .unwrap()
            .as_ref()
            .vruntime
            .load(Ordering::Relaxed) // lecture locale : Relaxed OK
    };
    // FIX SCHED-06 : Release pour visibilité cross-CPU (load balancer lit
    // min_vruntime depuis un autre CPU avec Acquire).
    self.min_vruntime.store(new_min, Ordering::Release);
}
```

---

## SCHED-07

**Fichier :** `docs/kernel/scheduler/SCHEDULER_CORE.md`  
**Sévérité :** P2 documentation — TCB et ThreadId incorrects  

### Analyse

| Champ | Documentation | Implémentation réelle |
|-------|--------------|----------------------|
| Taille TCB | **128 bytes** | **256 bytes** (`size_of::<ThreadControlBlock>() == 256`) |
| `ThreadId` | `pub struct ThreadId(pub u32)` | `pub struct ThreadId(pub u64)` |
| Offsets TCB | pid à [4], cpu à [8] (AtomicU32) | tid: u64 à [0], kstack_ptr: u64 à [8] |
| `ThreadAiState` | décrit comme champ TCB | n'existe pas dans le TCB |

La documentation est complètement désynchronisée depuis au moins l'architecture v7.

### Correctif — Remplacement des sections concernées dans `SCHEDULER_CORE.md`

```markdown
### ThreadControlBlock — Layout mémoire

`#[repr(C, align(64))]` — exactement **256 octets** (vérifié compile-time par
`const _: () = assert!(size_of::<ThreadControlBlock>() == 256)`).

```
Cache-line 1 [0..64]   — hot path pick_next_task()
  [0]   tid:          u64         identifiant thread (ThreadId.0)
  [8]   kstack_ptr:   u64         RSP kernel  ← switch_asm.s OFFSET HARDCODÉ
  [16]  priority:     Priority    (u8)
  [17]  policy:       SchedPolicy (u8)
  [18]  _pad0:        [u8; 6]
  [24]  sched_state:  AtomicU64   encodage compact état/flags/signal
  [32]  vruntime:     AtomicU64   vruntime CFS (ns)
  [40]  deadline_abs: AtomicU64   deadline EDF absolue (ns depuis boot)
  [48]  cpu_affinity: AtomicU64   bitmask affinité CPU (CPUs 0–63)
  [56]  cr3_phys:     u64         CR3 espace adressage  ← switch_asm.s OFFSET HARDCODÉ

Cache-line 2 [64..128]  — warm (context switch)
  [64]  cpu_id:       AtomicU64   CPU courant
  [72]  fs_base:      u64         FS.base (TLS userspace)
  [80]  user_gs_base: u64         GS.base userspace (KERNEL_GS_BASE)
  [88]  pkrs:         u32         PKRS (Intel PKS)
  [92]  pid:          ProcessId   identifiant processus (u32)
  [96]  signal_mask:  AtomicU64   bitmask signaux bloqués
  [104] dl_runtime:   u64         budget EDF (ns/période)
  [112] dl_period:    u64         période EDF (ns)
  [120] _pad2:        [u8; 8]

Cache-lines 3-4 [128..256] — cold
  [128] run_time_acc:     u64    temps CPU accumulé (ns)
  [136] switch_count:     u64    nombre de context switches
  [144] _cold_reserve:    [u8; 88]  extensions ExoShield + affinité étendue
  [232] fpu_state_ptr:    u64    pointeur XSAVE area  ← ExoPhoenix HARDCODÉ
  [240] rq_next:          u64    intrusive RunQueue (next)
  [248] rq_prev:          u64    intrusive RunQueue (prev)
```

### Identifiants (version corrigée)

```rust
pub struct ThreadId(pub u64);   // ← u64, pas u32
pub struct ProcessId(pub u32);
pub struct CpuId(pub u32);
```
```

---

# MODULE MEMORY

---

## MEM-01

**Fichier :** `kernel/src/arch/x86_64/paging.rs`  
**Sévérité :** P0 CRITIQUE — identique à ARCH-01, confirmé de l'angle `memory/`  

> Ce bogue est détaillé dans [ARCH-01](#arch-01). Sa description depuis la perspective `memory/` : tous les chemins qui passent par `memory::virt::page_table::x86_64::map_user_page()` (et équivalents) appellent finalement `arch/x86_64/paging::map_4k_page()`. Le correctif ARCH-01 s'applique et résout le problème pour les deux modules.

---

## MEM-02

**Fichier :** `kernel/src/memory/mod.rs`  
**Sévérité :** P2 mineur — commentaire API trompeur  

### Analyse

```rust
// DÉFAUT — memory/mod.rs, commentaire sur alloc_page re-export
// Note: alloc_zeroed_page n'existe pas dans physical — utiliser
// alloc_page() + écriture manuelle ou heap_alloc_zeroed() pour la heap.
```

`heap_alloc_zeroed()` n'est **pas** dans les re-exports de `memory/mod.rs`. Un développeur qui suit ce conseil obtient une erreur de compilation. La fonction publique correcte est `heap_alloc(layout)` suivi de `core::ptr::write_bytes(ptr, 0, size)`, ou l'utilisation du `#[global_allocator]` via `alloc::alloc::alloc_zeroed`.

### Correctif

```rust
// CORRECTION — kernel/src/memory/mod.rs

// Heap — allocateur global (SLUB / vmalloc).
// Note: pour allouer une page physique zéro-initialisée, utiliser :
//   let frame = alloc_page(AllocFlags::ZEROED)?;   // si flag ZEROED supporté
//   ou :
//   let frame = alloc_page(AllocFlags::NONE)?;
//   // puis remplir manuellement : ptr::write_bytes(phys_to_virt(frame) as *mut u8, 0, PAGE_SIZE)
// heap_alloc_zeroed() N'EXISTE PAS dans ce module.
// Pour la heap Rust standard : utiliser alloc::alloc::alloc_zeroed(layout).
pub use heap::{drain_on_context_switch, drain_on_memory_pressure, heap_alloc, heap_free};
```

---

# DOCS / SPÉCIFICATIONS FORMELLES TLA+

---

## TLA-01

**Fichier :** `docs/Exo-OS-TLA+/ContextSwitch.tla`  
**Sévérité :** P1 MAJEUR — divergence spec/code sur `CR0.TS` (Lazy FPU bit)  

### Analyse

Le modèle TLA+ `ContextSwitch.tla` décrit la séquence de context switch en 11 étapes. L'étape `Step2_SetLazyBit` positionne `Cr0TsBit = TRUE` (CR0.TS=1) **avant** le switch ASM (`Step5_AsmSwitch`) :

```tla
Step2_SetLazyBit(c) ==
    /\ SwitchStage[c] = 2
    /\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = TRUE]   ← AVANT Step5_AsmSwitch
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 3]
```

Mais l'implémentation Rust `context_switch()` positionne `CR0.TS = 1` **après** le switch ASM (étape 6/10 dans le code) :

```rust
// switch.rs — l'ordre réel dans context_switch()
// Étape 1 : FPU save (xsave)
// Étapes 2–4 : PKRS, FS/GS, état prev
// Étape 5 : context_switch_asm(...)   ← SWITCH ASM
// ────── contexte next à partir d'ici ──────
// Étape 6 : KPTI refresh
// Étape 7 : Restore PKRS
// Étape 8 : Restore CET SSP
// Étape 9 : TSS.RSP0 + set_current_tcb
// Étape 10: cr0_set_ts() + set_fpu_loaded(false)  ← CR0.TS = 1 ICI
```

L'invariant TLA+ `S25_STRESS_IrqFpuSafety` (`~Cr0TsBit[c] => FpuRegisters[c] = CurrentTcb[c].kstack_ptr`) modélise la sécurité FPU basée sur l'hypothèse que CR0.TS est mis à 1 tôt (avant le switch). Dans le code Rust, il y a une fenêtre entre le switch ASM et `cr0_set_ts()` où :
- Le CPU exécute maintenant `next`
- CR0.TS = 0 (FPU semble "chargée")
- Mais `next.fpu_loaded() == true` (hérité de la valeur précédente de `prev`)
- Si un IRQ survient dans cette fenêtre et déclenche une utilisation FPU, le handler #NM ne sera pas déclenché (CR0.TS = 0) → l'état FPU de `next` sera corrompu par l'IRQ sans détection.

### Impact réel

Cette fenêtre est extrêmement courte (~10 instructions) et les IRQ ne font généralement pas d'opérations FPU. Cependant, **le modèle TLA+ ne prouve pas les propriétés du code réel** — les invariants S25–S28 sont vérifiés sur un modèle inexact.

### Correctif — deux options

**Option A (code) :** avancer `cr0_set_ts()` avant le switch ASM (conforme au modèle TLA+) :

```rust
// CORRECTION TLA-01 Option A — switch.rs
// Déplacer cr0_set_ts() avant context_switch_asm() :

    // ── Étape 1 : Lazy FPU save ────────────────────────────────────────
    if prev.fpu_loaded() {
        fpu::save_restore::xsave_current(prev);
    }

    // FIX TLA-01 Option A : mettre CR0.TS = 1 ICI (avant ASM switch),
    // cohérent avec Step2_SetLazyBit du modèle TLA+.
    // La FPU de prev est déjà sauvée (étape 1). Si un IRQ arrive entre
    // ici et le switch ASM et utilise la FPU → #NM → lazy load de prev
    // (safe car save déjà fait).
    unsafe { fpu::lazy::cr0_set_ts(); }
    prev.set_fpu_loaded(false);
    // Marquer aussi next dès maintenant (sera le thread entrant)
    next.set_fpu_loaded(false);

    // ... suite inchangée jusqu'à context_switch_asm ...

    // Post-switch : SUPPRIMER l'appel cr0_set_ts() et set_fpu_loaded(false)
    // qui était en étape 6 car déjà effectué avant le switch.
```

**Option B (spec) :** mettre à jour `ContextSwitch.tla` pour refléter l'implémentation réelle :

```tla
(* FIX TLA-01 Option B : dans le modèle, CR0.TS est positionné APRÈS le switch ASM
   (Step6 et non Step2). Mettre à jour Step2 → pas de SetLazyBit ici, et
   ajouter Step6_SetLazyBit *)

(* Supprimer Step2_SetLazyBit *)
(* Remplacer Step6_7_Internal par : *)

Step6_SetLazyBit(c) ==
    /\ SwitchStage[c] = 6
    (* FIX TLA-01 : CR0.TS=1 sur le thread ENTRANT (CurrentTcb[c] = next) *)
    /\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = TRUE]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 7]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, FsBase, UserGsBase, GsSlot20,
                   FpuRegisters, XSaveArea, NextTcb>>

(* Mettre à jour Next pour inclure Step6_SetLazyBit *)
Next == \E c \in CORES :
    \/ SysUseFpu(c)
    \/ \E t \in TCB_SET : StartSwitch(c, t)
    \/ Step1_Xsave(c)
    (* Plus de Step2_SetLazyBit — supprimé *)
    \/ Step2_3_Internal(c)   (* renommer Step3_4_Internal *)
    \/ Step5_AsmSwitch(c)
    \/ Step6_SetLazyBit(c)   (* nouveau *)
    \/ Step7_Internal(c)
    \/ Step8_UpdateTss(c)
    \/ Step9_10_RestoreMSRs(c)
    \/ Step11_Finish(c)

(* Mettre à jour l'invariant S25 pour la nouvelle sémantique :
   la FPU est "unsafe" (CR0.TS=0, fpu_loaded=true) entre Step1_Xsave
   et Step6_SetLazyBit, mais pendant cette fenêtre CurrentTcb = prev
   (avant Step5_AsmSwitch) donc la FPU physique appartient encore à prev. *)
S25_STRESS_IrqFpuSafety ==
    \A c \in CORES :
        (~Cr0TsBit[c] /\ ~SwitchInProgress(c))
            => (FpuRegisters[c] = CurrentTcb[c].kstack_ptr)
```

> **Recommandation :** appliquer l'**Option A** (avancer `cr0_set_ts()`) car elle est plus sûre architecturalement et aligne le code avec le modèle formel sans avoir à le redéfinir.

---

## Récapitulatif des correctifs par priorité

### P0 — À appliquer immédiatement

| ID | Fichier | Action |
|----|---------|--------|
| ARCH-01 / MEM-01 | `paging.rs` | Ajouter `PTE_USER` dans `get_or_create_subtable()` + paramètre `is_user_mapping` |
| SCHED-01 | `switch.rs` | Incrémenter `switch_count` + `ctx_switch_count` dans `context_switch()` |
| SCHED-03 | `runqueue.rs` | `CfsRunQueue::enqueue()` → retourner `bool` + paniquer si thread perdu |

### P1 — À appliquer en priorité haute

| ID | Fichier | Action |
|----|---------|--------|
| ARCH-02 | `gdt.rs` | Appeler `validate_star_layout()` avec `assert!` dans `init_gdt_for_cpu()` |
| ARCH-03 | `switch.rs` | Corriger le commentaire du bloc `extern "C" context_switch_asm` |
| SCHED-02 | `switch.rs` | Mettre à jour `run_time_acc` avec le delta TSC dans `context_switch()` |
| SCHED-04 | `runqueue.rs` | Ajouter `picks_dl: AtomicU64` dans `RunQueueStats`, l'utiliser pour DEADLINE |
| TLA-01 | `ContextSwitch.tla` | Aligner le modèle TLA+ sur l'ordre réel (Option A ou B) |

### P2 — À traiter dans le prochain sprint de dette technique

| ID | Fichier | Action |
|----|---------|--------|
| ARCH-04 | `paging.rs` | Implémenter `InvalidAlignment`, `NotMapped`, ou les supprimer |
| ARCH-05 | `idt.rs` | Aliaser `VEC_IRQ_TIMER = IRQ_BASE` ; l'utiliser dans `init_idt()` |
| SCHED-05 | `switch.rs` | Supprimer `check_signal_pending(next)` mort en fin de `context_switch()` |
| SCHED-06 | `runqueue.rs` | Passer `min_vruntime.store(...)` à `Ordering::Release` |
| SCHED-07 | `SCHEDULER_CORE.md` | Mettre à jour layout TCB : 256 B, `ThreadId(u64)`, offsets réels |
| MEM-02 | `mod.rs` | Corriger le commentaire `heap_alloc_zeroed()` → procédure correcte |

---

*— claude-gamma, audit ExoOS modules arch/memory/scheduler, 2026-05-03*
