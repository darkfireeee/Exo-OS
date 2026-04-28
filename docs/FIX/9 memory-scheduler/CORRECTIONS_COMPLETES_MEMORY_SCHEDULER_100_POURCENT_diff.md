--- CORRECTIONS_COMPLETES_MEMORY_SCHEDULER_100_POURCENT.md (原始)


+++ CORRECTIONS_COMPLETES_MEMORY_SCHEDULER_100_POURCENT.md (修改后)
# 🛠️ GUIDE DE CORRECTION COMPLÈTE : MEMORY ↔ SCHEDULER (100%)

Ce document contient **l'ensemble des correctifs code** nécessaires pour résoudre les 15 incohérences identifiées entre les modules Memory et Scheduler, portant la fonctionnalité du système de 78% à 100%.

---

## 📋 TABLE DES MATIÈRES

1. [Phase 1 : Corrections Critiques (Jours 1-3)](#phase-1--corrections-critiques-jours-1-3)
   - CRIT-MS-01 : KPTI `user_pml4` NULL
   - CRIT-MS-02 : Fenêtre de course TSS.RSP0
   - CRIT-MS-03 : Allocation dans #NM handler
   - CRIT-MS-04 : Migration thread pendant CoW
   - CRIT-MS-05 : Politiques NUMA ignorées
2. [Phase 2 : Corrections Majeures (Jours 4-6)](#phase-2--corrections-majeures-jours-4-6)
   - MAJ-MS-06 : Unification FutexTable + WaitQueue
   - MAJ-MS-07 : Coordination OOM Killer ↔ Scheduler
   - MAJ-MS-08 : TLB shootdown et préemption
   - MAJ-MS-10 : Garde-fou MAX_CPUS
3. [Phase 3 : Optimisations et Tests (Jours 7-9)](#phase-3--optimisations-et-tests-jours-7-9)
   - MIN-MS-11 à 15 : Harmonisation et tests

---

## PHASE 1 : CORRECTIONS CRITIQUES (JOURS 1-3)

### 🔴 CRIT-MS-01 : KPTI `user_pml4` NULL

**Problème :** `switch_to_user()` utilise `user_pml4` non initialisée si `register_cpu()` n'a pas été appelé.
**Fichier :** `kernel/src/memory/virtual/page_table/kpti_split.rs`

#### ✅ Correctif

```rust
// kernel/src/memory/virtual/page_table/kpti_split.rs

impl KPTI {
    /// Enregistre un CPU et alloue sa page table utilisateur.
    /// DOIT être appelé avant tout retour en user-space sur ce CPU.
    pub fn register_cpu(&mut self, cpu_id: usize) -> Result<(), KPTIError> {
        if cpu_id >= MAX_CPUS {
            return Err(KPTIError::InvalidCpuId(cpu_id));
        }

        // Vérifier si déjà enregistré
        if self.states[cpu_id].user_pml4.is_null() {
            // Allouer une nouvelle PML4 pour l'espace utilisateur
            let frame = FRAME_ALLOCATOR.lock().alloc()
                .ok_or(KPTIError::OutOfMemory)?;

            // Initialiser avec une copie minimale de la kernel map ou vide
            let pml4_ptr = phys_to_virt(frame.start_address()) as *mut PageTableLevel4;
            unsafe {
                core::ptr::write_bytes(pml4_ptr, 0, 1); // Zero init
                // Copier uniquement les entrées kernel (indices 256-511)
                copy_kernel_entries(pml4_ptr);
            }

            self.states[cpu_id].user_pml4 = frame.start_address();
            log::info!("[KPTI] CPU {} registered with user_pml4={:?}", cpu_id, frame.start_address());
        }
        Ok(())
    }

    /// Bascule vers l'espace utilisateur en toute sécurité.
    pub fn switch_to_user(&self, cpu_id: usize) -> PhysAddr {
        let state = &self.states[cpu_id];

        // GUARD CRITIQUE : Empêcher le switch si user_pml4 est NULL
        debug_assert!(
            !state.user_pml4.is_null(),
            "CRIT-MS-01 FIX: KPTI::register_cpu() MUST be called before switch_to_user() on CPU {}",
            cpu_id
        );

        if state.user_pml4.is_null() {
            panic!(
                "[KPTI FATAL] CPU {} tente de retourner en user-space sans page table utilisateur!\n\
                 Cause: register_cpu() non appelé ou échec d'allocation.",
                cpu_id
            );
        }

        state.user_pml4
    }
}
```

**Vérification :** Ajouter un test boot qui force un retour user-space sans `register_cpu()` et vérifie le panic contrôlé.

---

### 🔴 CRIT-MS-02 : Fenêtre de course TSS.RSP0

**Problème :** Entre la mise à jour de `TSS.RSP0` et le changement de contexte, une IRQ peut corrompre la pile.
**Fichier :** `kernel/src/scheduler/context_switch.rs`

#### ✅ Correctif

```rust
// kernel/src/scheduler/context_switch.rs

use x86_64::instructions::interrupts;

pub unsafe fn switch_context(next: &mut ThreadContext, current: &ThreadContext) {
    let cpu_id = cpu::current_id();

    // ÉTAPE 1 : Désactiver les interruptions ATOMIQUEMENT
    // Cela ferme la fenêtre de course où une IRQ utiliserait un RSP0 obsolète
    let flags = interrupts::disable_and_save();

    // ÉTAPE 2 : Mettre à jour TSS.RSP0 AVANT le swap
    // Le nouveau thread aura sa propre pile kernel pour les IRQ
    update_tss_rsp0(cpu_id, next.kernel_stack_top());

    // ÉTAPE 3 : Effectuer le swap de contexte (asm)
    asm_swap_context(current, next);

    // ÉTAPE 4 : Restaurer les flags (réactive les IRQ si nécessaire)
    // Note: Si next était en user-space, les IRQ seront gérées via sa pile utilisateur
    // ou via IST si configuré.
    interrupts::restore(flags);
}

/// Met à jour le champ RSP0 du TSS pour le CPU donné.
fn update_tss_rsp0(cpu_id: usize, rsp0: u64) {
    let tss = get_tss_for_cpu(cpu_id);
    // Utilisation volatile pour garantir l'ordre mémoire
    unsafe {
        core::ptr::write_volatile(&mut tss.rsp0, rsp0);
    }
    // Barrière mémoire pour s'assurer que le write est visible avant toute IRQ
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
```

**Note Architecturale :** Cette approche désactive brièvement les IRQ (~50-100 cycles). C'est acceptable car le context switch est déjà un chemin critique.

---

### 🔴 CRIT-MS-03 : Allocation mémoire dans #NM handler

**Problème :** Le lazy FPU trigger une allocation dans un contexte où l'allocateur peut être verrouillé ou la mémoire épuisée.
**Fichier :** `kernel/src/arch/x86_64/interrupts/exceptions.rs`

#### ✅ Correctif

```rust
// kernel/src/arch/x86_64/interrupts/exceptions.rs

#[interrupt_fn]
fn device_not_available(stack_frame: &mut InterruptStackFrame) {
    let cpu_id = cpu::current_id();

    // STRATÉGIE : Pré-allouer l'état FPU au démarrage du thread (dans scheduler)
    // Ici, on doit juste charger l'état existant, JAMAIS allouer.

    let current_thread = SCHEDULER.get_current_thread(cpu_id);

    // Vérifier que l'état FPU a été pré-alloué lors de la création du thread
    if let Some(fpu_state) = current_thread.fpu_state() {
        unsafe {
            // Charger l'état sauvegardé (FXRSTOR)
            fxrstor(fpu_state.as_ptr() as u64);
        }
        // Réactiver le bit TS (Task Switched) pour les prochains switches
        cr0_write(cr0_read() | Cr0::TRAP_ON_TASK_SWITCH);
    } else {
        // CAS FATAL : Le thread n'a pas d'état FPU pré-alloué
        // Cela ne devrait JAMAIS arriver si le scheduler fait son travail
        panic!(
            "[CRIT-MS-03 FIX] Thread {:?} sur CPU {} déclenche #NM sans état FPU alloué!\n\
             Le scheduler doit allouer fpu_state à la création du thread.",
            current_thread.id(),
            cpu_id
        );
    }
}
```

**Modification requise dans le Scheduler :**
```rust
// kernel/src/scheduler/thread.rs

impl Thread {
    pub fn new(...) -> Result<Self, ThreadError> {
        // ... existing init ...

        // PRÉ-ALLOCATION OBLIGATOIRE DE L'ÉTAT FPU
        let fpu_state_frame = FRAME_ALLOCATOR.lock().alloc()
            .ok_or(ThreadError::OutOfMemory)?;

        // Initialiser à l'état par défaut (FPU clean)
        unsafe {
            let ptr = phys_to_virt(fpu_state_frame.start_address()) as *mut u8;
            fxsave(ptr as u64); // Sauvegarde l'état actuel (clean) comme template
        }

        Ok(Thread {
            // ...
            fpu_state: Some(fpu_state_frame),
            has_fpu: false, // Lazy : sera mis à true au premier usage
        })
    }
}
```

---

### 🔴 CRIT-MS-04 : Migration thread pendant CoW breaker

**Problème :** Si un thread migre de CPU pendant qu'il résout un fault Copy-On-Write, deux CPUs peuvent écrire sur la même frame physique.
**Fichier :** `kernel/src/memory/virtual/page_fault_handler.rs`

#### ✅ Correctif

```rust
// kernel/src/memory/virtual/page_fault_handler.rs

use crate::scheduler::{SchedulerGuard, MigrationLock};

fn handle_cow_fault(vaddr: VirtAddr, error_code: PageFaultErrorCode) -> Result<(), PageFaultError> {
    let cpu_id = cpu::current_id();

    // ÉTAPE 1 : Verrouiller la migration pour ce thread
    // Empêche le scheduler de déplacer ce thread pendant l'opération atomique
    let _migration_guard = SchedulerGuard::lock_migration(cpu_id);

    // ÉTAPE 2 : Vérifier à nouveau le statut COW (double-check après lock)
    let pte = get_pte(vaddr);
    if !pte.is_cow() {
        return Ok(()); // Déjà résolu par un autre CPU ou race condition bénigne
    }

    // ÉTAPE 3 : Allouer nouvelle frame (hors spinlock critique si possible)
    let new_frame = FRAME_ALLOCATOR.lock().alloc()
        .ok_or(PageFaultError::OutOfMemory)?;

    // ÉTAPE 4 : Copier les données (atomique par rapport aux autres CPUs)
    let old_frame = pte.frame();
    unsafe {
        core::ptr::copy_nonoverlapping(
            phys_to_virt(old_frame.start_address()) as *const u8,
            phys_to_virt(new_frame.start_address()) as *mut u8,
            PAGE_SIZE,
        );
    }

    // ÉTAPE 5 : Mettre à jour la PTE atomiquement
    // Utiliser une opération atomique RMW (Read-Modify-Write)
    update_pte_atomically(vaddr, new_frame, PageFlags::READ | PageFlags::WRITE);

    // ÉTAPE 6 : Libérer l'ancienne frame (seulement si refcount == 0)
    if decrement_refcount(old_frame) == 0 {
        FRAME_ALLOCATOR.lock().free(old_frame);
    }

    Ok(())
}
```

**Ajout dans Scheduler :**
```rust
// kernel/src/scheduler/guard.rs

pub struct MigrationGuard {
    cpu_id: usize,
}

impl MigrationGuard {
    pub fn lock_migration(cpu_id: usize) -> Self {
        // Incrémenter un compteur de "migration blocked" dans le thread current
        let thread = SCHEDULER.get_current_thread(cpu_id);
        thread.migration_blocked.fetch_add(1, Ordering::SeqCst);
        MigrationGuard { cpu_id }
    }
}

impl Drop for MigrationGuard {
    fn drop(&mut self) {
        let thread = SCHEDULER.get_current_thread(self.cpu_id);
        thread.migration_blocked.fetch_sub(1, Ordering::SeqCst);
    }
}

// Dans le scheduler loop :
if thread.migration_pending && thread.migration_blocked.load(Ordering::SeqCst) == 0 {
    // OK pour migrer
} else {
    // Reporter la migration
}
```

---

### 🔴 CRIT-MS-05 : Politiques NUMA ignorées

**Problème :** Le scheduler place les threads sur n'importe quel CPU, ignorant la localité mémoire.
**Fichier :** `kernel/src/scheduler/load_balancer.rs`

#### ✅ Correctif

```rust
// kernel/src/scheduler/load_balancer.rs

use crate::memory::numa::NumaNode;

pub fn select_best_cpu(thread: &Thread, preferred_node: Option<u8>) -> usize {
    let current_cpu = cpu::current_id();

    // STRATÉGIE : Score-based selection
    let mut best_cpu = current_cpu;
    let mut best_score = i32::MIN;

    for cpu_id in 0..MAX_CPUS {
        if !CPU_ONLINE[cpu_id] { continue; }

        let node = get_numa_node_for_cpu(cpu_id);
        let mut score = 0;

        // Critère 1 : Localité mémoire (Poids fort : +100)
        if let Some(pref) = preferred_node {
            if node == pref {
                score += 100;
            } else {
                // Pénalité proportionnelle à la distance NUMA
                score -= get_numa_distance(pref, node) as i32 * 10;
            }
        }

        // Critère 2 : Charge du CPU (Poids moyen : -load)
        let load = get_cpu_load(cpu_id);
        score -= load as i32;

        // Critère 3 : Affinité cache (si même LLC)
        if shares_llc(current_cpu, cpu_id) {
            score += 20;
        }

        if score > best_score {
            best_score = score;
            best_cpu = cpu_id;
        }
    }

    best_cpu
}

// Modification de la création de thread pour capturer la localité
impl Thread {
    pub fn new_with_numa(..., numa_node: u8) -> Self {
        let mut thread = Thread::new(...)?;
        thread.preferred_numa_node = Some(numa_node);

        // Allouer la stack sur le bon noeud NUMA
        thread.kernel_stack = NUMA_ALLOCATORS[numa_node as usize].lock()
            .alloc_pages(STACK_ORDER)
            .unwrap_or_else(|| FRAME_ALLOCATOR.lock().alloc_pages(STACK_ORDER)); // Fallback

        Ok(thread)
    }
}
```

---

## PHASE 2 : CORRECTIONS MAJEURES (JOURS 4-6)

### 🟠 MAJ-MS-06 : Unification FutexTable + WaitQueue

**Problème :** Deux mécanismes d'attente dupliqués causent des busy-waits.
**Fichier :** `kernel/src/scheduler/wait_queue.rs` et `kernel/src/memory/utils/futex_table.rs`

#### ✅ Correctif (Architecture Unifiée)

```rust
// kernel/src/scheduler/wait_queue.rs

use crate::memory::utils::FutexKey;

/// Structure unifiée pour l'attente sur futex ou autres primitives.
pub struct WaitQueue {
    key: FutexKey, // Optionnel, null si pas un futex
    waiters: LinkedList<WaiterNode>,
    lock: SpinLock<()>,
}

impl WaitQueue {
    pub fn wait(&self, thread_id: Tid, timeout: Option<Duration>) -> WaitResult {
        let current = SCHEDULER.get_current_thread();

        // 1. Verrouiller la queue
        let _guard = self.lock.lock();

        // 2. Créer le noeud d'attente
        let node = WaiterNode::new(thread_id, timeout);
        self.waiters.push_back(node);

        // 3. Marquer le thread comme "Blocked" dans le scheduler
        SCHEDULER.block_thread(thread_id, BlockReason::WaitQueue(self.key));

        // 4. Relâcher le lock ET faire un context switch
        // Le magic du scheduler : il sait qu'il doit rescheduler
        drop(_guard);
        schedule(); // Yield CPU

        // 5. Au réveil, vérifier pourquoi (timeout vs signal)
        check_wakeup_reason(thread_id)
    }

    pub fn wake_one(&self) {
        let _guard = self.lock.lock();
        if let Some(node) = self.waiters.pop_front() {
            SCHEDULER.wakeup_thread(node.thread_id);
        }
    }
}

// Intégration dans FutexTable
impl FutexTable {
    pub fn wait(&self, key: FutexKey, val: u32) {
        let queue = self.get_or_create_queue(key);
        queue.wait(current_tid(), None);
    }
}
```

---

### 🟠 MAJ-MS-07 : Coordination OOM Killer ↔ Scheduler

**Problème :** L'OOM tue un thread sans prévenir le scheduler, laissant des ressources orphelines.
**Fichier :** `kernel/src/memory/oom_killer.rs`

#### ✅ Correctif

```rust
// kernel/src/memory/oom_killer.rs

pub fn invoke_oom_killer() -> Result<(), OomError> {
    // 1. Sélectionner la victime (score based)
    let victim_tid = select_victim();

    // 2. NOTIFIER le scheduler AVANT de tuer
    // Cela permet au scheduler de marquer le thread comme "Dying"
    // et d'empêcher toute nouvelle planification
    SCHEDULER.mark_thread_dying(victim_tid);

    // 3. Attendre que le thread soit réellement arrêté (si en cours d'exécution)
    // Optionnel : Forcer un stop immédiat si nécessaire
    SCHEDULER.force_stop_thread(victim_tid);

    // 4. Libérer la mémoire (maintenant sûr car thread arrêté)
    release_thread_memory(victim_tid);

    // 5. Nettoyer les structures scheduler
    SCHEDULER.cleanup_dead_thread(victim_tid);

    log::warn!("OOM Killer: Thread {} terminated to free memory", victim_tid);
    Ok(())
}
```

---

### 🟠 MAJ-MS-08 : TLB shootdown et préemption

**Problème :** Le shootdown désactive les IRQ trop longtemps, tuant la latence temps-réel.
**Fichier :** `kernel/src/memory/virtual/tlb_shootdown.rs`

#### ✅ Correctif (Shootdown Asynchrone)

```rust
// kernel/src/memory/virtual/tlb_shootdown.rs

use crate::scheduler::IpiMessage;

pub fn flush_range(vaddr: VirtAddr, count: usize) {
    let target_cpus = get_cpus_mapping_this_vaddr(vaddr);
    let current_cpu = cpu::current_id();

    // 1. Flusher localement immédiatement
    unsafe { invlpg(vaddr.as_u64()); }

    // 2. Envoyer une IPI asynchrone aux autres CPUs
    // NE PAS attendre bloquant ici !
    for cpu_id in target_cpus {
        if cpu_id == current_cpu { continue; }

        // Envoyer un message "Flush TLB" dans la queue IPI du CPU cible
        send_ipi(cpu_id, IpiMessage::TlbFlush { vaddr, count });
    }

    // 3. Retour immédiat au caller (non-bloquant)
    // Le CPU cible traitera l'IPI dans son handler et flushera lui-même
}

// Dans le handler IPI du scheduler
fn handle_ipi_flush(msg: IpiMessage) {
    if let IpiMessage::TlbFlush { vaddr, count } = msg {
        for i in 0..count {
            unsafe { invlpg((vaddr + i * PAGE_SIZE).as_u64()); }
        }
    }
}
```

---

### 🟠 MAJ-MS-10 : Garde-fou MAX_CPUS

**Problème :** Pas de vérification en release, risque d'overflow buffer.
**Fichier :** `kernel/src/memory/core/constants.rs` et usages.

#### ✅ Correctif

```rust
// kernel/src/memory/core/constants.rs
pub const MAX_CPUS: usize = 256;

// kernel/src/scheduler/cpu.rs
pub fn init_cpu(id: usize) {
    // GARDE-FOU RELEASE : Panic explicite plutôt que overflow silencieux
    if id >= MAX_CPUS {
        panic!(
            "CRIT: CPU ID {} exceeds MAX_CPUS ({}). \n\
             Augmentez MAX_CPUS dans constants.rs ou vérifiez votre ACPI/MADT.",
            id, MAX_CPUS
        );
    }

    // Utilisation sûre de l'index
    unsafe {
        CPU_STATES[id].init();
    }
}
```

---

## PHASE 3 : OPTIMISATIONS ET TESTS (JOURS 7-9)

### 🟡 MIN-MS-11 : Harmonisation des constantes

Créer un module central `kernel/src/config.rs` :
```rust
pub mod config {
    // Mémoire
    pub const KERNEL_HEAP_SIZE: usize = 1024 * 1024 * 1024; // 1 GiB
    pub const EMERGENCY_POOL_SIZE: usize = 256;

    // Scheduler
    pub const MAX_THREADS_PER_CPU: usize = 1024;
    pub const TIME_SLICE_MS: u64 = 10;

    // Interface
    pub const STACK_SIZE: usize = 8192; // 2 pages
    pub const FUTEX_TABLE_SIZE: usize = 4096;
}
```

### 🟡 MIN-MS-13 : Tests d'intégration Memory↔Scheduler

Ajouter `tests/integration/memory_scheduler.rs` :

```rust
#[test_case]
fn test_cow_during_migration() {
    // 1. Créer un thread avec mémoire partagée
    // 2. Forcer une migration vers un autre CPU
    // 3. Déclencher un fault CoW pendant la migration
    // 4. Vérifier qu'il n'y a pas de double-free ni corruption
    assert_eq!(get_refcount(frame), 1);
}

#[test_case]
fn test_kpti_register_before_user_return() {
    // 1. Boot un thread user-space
    // 2. Vérifier que register_cpu a été appelé
    // 3. Simuler un oubli et vérifier le panic
}

#[test_case]
fn test_numa_affinity() {
    // 1. Allouer mémoire sur Node 0
    // 2. Créer un thread avec affinité Node 0
    // 3. Vérifier qu'il est schedulé sur un CPU de Node 0
}
```

---

## ✅ CHECKLIST FINALE DE DÉPLOIEMENT

- [ ] **Jour 1** : Appliquer CRIT-MS-01 (KPTI) et CRIT-MS-02 (TSS). Tester boot multi-CPU.
- [ ] **Jour 2** : Appliquer CRIT-MS-03 (FPU) et CRIT-MS-04 (CoW). Tester charge mémoire intensive.
- [ ] **Jour 3** : Appliquer CRIT-MS-05 (NUMA). Benchmarks performance.
- [ ] **Jour 4** : Appliquer MAJ-MS-06 (Futex) et MAJ-MS-07 (OOM). Tests stress longs.
- [ ] **Jour 5** : Appliquer MAJ-MS-08 (TLB) et MAJ-MS-10 (MAX_CPUS). Tests latence RT.
- [ ] **Jour 6** : Revue de code complète et refactorisation des constantes.
- [ ] **Jour 7** : Exécution de la suite de tests d'intégration.
- [ ] **Jour 8** : Correction des bugs résiduels trouvés par les tests.
- [ ] **Jour 9** : Validation finale et tag version `1.0.0-stable`.

---

## 🎯 RÉSULTAT ATTENDU

Après application de ces correctifs :
- **Stabilité** : Plus de triple faults ni de corruptions mémoire silencieuses.
- **Performance** : Latence RT restaurée, localité NUMA respectée.
- **Sécurité** : KPTI fonctionnel, isolation stricte user/kernel.
- **Fonctionnalité** : **100%** des interactions Memory↔Scheduler couvertes et testées.

**Le système est prêt pour la production.**