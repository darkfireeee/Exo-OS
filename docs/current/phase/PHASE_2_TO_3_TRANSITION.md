# Phase 2 → Phase 3 - Plan de Transition

**Date**: 1er janvier 2026  
**Phase 2 Status**: ✅ SMP Infrastructure Complete  
**Phase 3 Status**: 🔄 Ready to Begin  

---

## ✅ Phase 2 - Accomplissements

### Infrastructure SMP (100% ✅)
- ✅ ACPI/MADT parsing - 4 CPUs détectés
- ✅ Local APIC init - Par CPU
- ✅ I/O APIC init - Routage IRQ
- ✅ IPI messaging - INIT/SIPI avec delivery verification
- ✅ AP Bootstrap - Trampoline 16→32→64 bit
- ✅ SSE/FPU/AVX init - Sur tous les CPUs
- ✅ Per-CPU structures - CpuInfo isolé
- ✅ Tests Bochs - 4/4 CPUs online

### Organisation Code (100% ✅)
- ✅ Nettoyage dossier smp/ - 5 fichiers obsolètes supprimés
- ✅ Création dossier utils/ - Modules organisés
- ✅ Mise à jour imports - Tous les chemins corrects
- ✅ Documentation complète - PHASE_2_SMP_COMPLETE.md

---

## 🎯 Phase 3 - Objectifs

### Scheduler Multi-Core (Priorité #1)

#### 3.1 - Run Queues Per-CPU
**Objectif**: Chaque CPU a sa propre queue de threads

**Tâches**:
1. [ ] Implémenter PerCpuScheduler structure
2. [ ] Queue locale par CPU (lock-free ou avec spinlock)
3. [ ] Intégrer avec SCHEDULER global
4. [ ] Migration API: move_thread(thread, target_cpu)

**Fichiers à modifier**:
- `kernel/src/scheduler/core/per_cpu.rs`
- `kernel/src/scheduler/mod.rs`
- `kernel/src/arch/x86_64/smp/mod.rs`

**Critères de succès**:
- ✅ Chaque CPU peut pick_next() depuis sa queue locale
- ✅ Threads peuvent être ajoutés à un CPU spécifique
- ✅ Stats par CPU (running, idle, context_switches)

---

#### 3.2 - Load Balancing
**Objectif**: Distribution équitable des threads sur tous les CPUs

**Algorithmes à implémenter**:

1. **Initial Placement**:
   ```rust
   fn choose_target_cpu(thread: &Thread) -> usize {
       // Stratégie: least loaded CPU
       let mut min_load = usize::MAX;
       let mut target = 0;
       
       for cpu in 0..cpu_count() {
           let load = get_cpu_load(cpu);
           if load < min_load {
               min_load = load;
               target = cpu;
           }
       }
       target
   }
   ```

2. **Work Stealing** (idle CPU steal from busy):
   ```rust
   fn try_steal_work(idle_cpu: usize) -> Option<Arc<Thread>> {
       for victim_cpu in 0..cpu_count() {
           if victim_cpu == idle_cpu { continue; }
           
           if let Some(thread) = try_steal_from(victim_cpu) {
               return Some(thread);
           }
       }
       None
   }
   ```

3. **Periodic Rebalancing** (every 100ms):
   - Calculer load average par CPU
   - Si imbalance > 25%: migrer threads
   - Éviter ping-pong (migration trop fréquente)

**Critères de succès**:
- ✅ Load variance < 20% entre CPUs
- ✅ Pas de CPU idle si work disponible
- ✅ Migration overhead < 5% CPU time

---

#### 3.3 - CPU Affinity
**Objectif**: Contrôle fin de placement threads

```rust
pub struct ThreadAffinity {
    allowed_cpus: CpuSet,  // Bitmap de CPUs autorisés
    preferred_cpu: Option<usize>,
}

impl Thread {
    pub fn set_affinity(&mut self, cpus: CpuSet) {
        self.affinity.allowed_cpus = cpus;
    }
    
    pub fn pin_to_cpu(&mut self, cpu: usize) {
        self.affinity.allowed_cpus = CpuSet::single(cpu);
        self.affinity.preferred_cpu = Some(cpu);
    }
}
```

**Use cases**:
- Threads RT: pin à CPU dédié
- Threads I/O: affinity vers CPU proche du device
- User threads: configurable via syscall

---

#### 3.4 - Interrupts sur APs
**Objectif**: Activer interruptions sur Application Processors

**Modifications à faire**:

1. **ap_startup()**: Ajouter `sti` après init complète
   ```rust
   // Stage 10: Enable interrupts
   unsafe {
       core::arch::asm!("sti");
   }
   log::info!("[AP {}] Interrupts enabled", cpu_id);
   ```

2. **Timer interrupts**: APIC timer par CPU
   - Déjà configuré dans ap_startup()
   - Vérifier IRQ routing correct
   - Tester preemption sur chaque CPU

3. **IPI interrupts**: Vecteur dédié (vector 253)
   ```rust
   // Envoyer IPI de reschedule à CPU distant
   pub fn send_reschedule_ipi(target_cpu: usize) {
       let apic_id = SMP_SYSTEM.cpus[target_cpu].apic_id.load();
       send_ipi(apic_id, IPI_RESCHEDULE_VECTOR);
   }
   ```

**Critères de succès**:
- ✅ Timer IRQ sur chaque CPU (100Hz)
- ✅ Preemption fonctionne sur tous les CPUs
- ✅ IPI réveille CPU idle instantanément

---

### Logging Lock-Free (Priorité #2)

**Problème actuel**: 
- Serial port a un verrou global
- Appels `log::()` depuis AP causent deadlock
- Solution temporaire: port 0xE9 uniquement

**Solution permanente**:

1. **Ring Buffer Per-CPU**:
   ```rust
   pub struct PerCpuLogBuffer {
       buffer: [u8; 4096],
       write_pos: AtomicUsize,
       cpu_id: u32,
   }
   
   static CPU_LOG_BUFFERS: [PerCpuLogBuffer; MAX_CPUS] = ...;
   ```

2. **Background Flush Thread** (sur BSP):
   ```rust
   fn log_flusher() {
       loop {
           for cpu in 0..cpu_count() {
               let buf = &CPU_LOG_BUFFERS[cpu];
               flush_to_serial(buf);
           }
           sleep_ms(10);
       }
   }
   ```

3. **Macro log!() modifié**:
   ```rust
   macro_rules! log {
       ($fmt:expr, $($args:tt)*) => {{
           let cpu_id = current_cpu_id();
           let buf = &CPU_LOG_BUFFERS[cpu_id];
           buf.write_fmt(format_args!($fmt, $($args)*));
       }}
   }
   ```

**Avantages**:
- ✅ Pas de contention lock
- ✅ Overhead minimal (<50 cycles)
- ✅ Log complet depuis tous les CPUs
- ✅ Ordre temporel préservé (timestamps)

---

### TLB Shootdown (Priorité #3)

**Objectif**: Synchroniser invalidation TLB sur tous les CPUs

**Problème**:
- Un CPU modifie page table
- Autres CPUs ont encore ancien mapping en TLB
- Risque de corruption mémoire

**Solution**:

1. **IPI TLB Shootdown**:
   ```rust
   pub fn tlb_shootdown(addr: VirtAddr, cpus: CpuSet) {
       // 1. Envoyer IPI à chaque CPU dans le set
       for cpu in cpus.iter() {
           send_tlb_shootdown_ipi(cpu, addr);
       }
       
       // 2. Attendre ACK de tous les CPUs
       wait_for_tlb_acks(cpus);
       
       // 3. Invalider TLB local
       invlpg(addr);
   }
   
   // Handler IPI TLB shootdown
   pub extern "C" fn tlb_shootdown_handler() {
       let addr = TLB_SHOOTDOWN_ADDR.load();
       invlpg(addr);
       TLB_SHOOTDOWN_ACK.fetch_add(1);
       send_eoi();
   }
   ```

2. **Optimisations**:
   - Batch multiple addresses
   - Éviter shootdown si CPU pas actif
   - Utiliser PCID pour isolation

**Critères de succès**:
- ✅ Pas de corruption après fork/exec
- ✅ Latency < 10μs pour 4 CPUs
- ✅ Tests concurrent mmap/munmap passent

---

## 📊 Métriques de Succès Phase 3

### Performance
```
Métrique                  Cible        Actuel
──────────────────────────────────────────────
Load balance variance    < 20%         N/A
CPU idle avec work       0%            N/A
Migration overhead       < 5%          N/A
Context switch per-CPU   < 2000 cycles ~2000
IPI latency              < 10μs        ~30μs
TLB shootdown (4 CPUs)   < 10μs        N/A
```

### Scalabilité
```
CPUs    Throughput    Latency    Overhead
─────────────────────────────────────────────
1       100%          baseline   0%
2       190%          +5%        5%
4       360%          +10%       10%
8       680%          +15%       15%
```

**Target**: Efficacité > 90% jusqu'à 8 CPUs

---

## 🗺️ Timeline Phase 3

### Semaine 1: Scheduler SMP (5-7 jours)
- Jour 1-2: Run queues per-CPU
- Jour 3-4: Load balancing basique
- Jour 5: CPU affinity
- Jour 6-7: Tests et debug

### Semaine 2: Optimisations (5-7 jours)
- Jour 1-2: Work stealing
- Jour 3-4: Logging lock-free
- Jour 5: Interrupts sur APs
- Jour 6-7: Tests stress

### Semaine 3: TLB & Polish (3-5 jours)
- Jour 1-2: TLB shootdown
- Jour 3: Documentation
- Jour 4-5: Tests finaux, benchmarks

**Durée totale estimée**: 2-3 semaines

---

## 🚀 Prochaine Action Immédiate

### Action #1: Activer interruptions sur APs (30 min)

**Pourquoi d'abord?**
- Simple modification (1 ligne: `sti`)
- Permet de tester preemption multi-CPU
- Nécessaire pour scheduler SMP

**Code**:
```rust
// Dans ap_startup(), après Stage 9
unsafe {
    core::arch::asm!("sti");
}
```

### Action #2: Implémenter PerCpuScheduler (2-3h)

**Structure**:
```rust
pub struct PerCpuScheduler {
    cpu_id: usize,
    run_queue: VecDeque<Arc<Thread>>,
    current: Option<Arc<Thread>>,
    idle_thread: Arc<Thread>,
    stats: CpuStats,
}

impl PerCpuScheduler {
    pub fn pick_next(&mut self) -> Arc<Thread> {
        self.run_queue.pop_front()
            .unwrap_or_else(|| self.idle_thread.clone())
    }
    
    pub fn add_thread(&mut self, thread: Arc<Thread>) {
        self.run_queue.push_back(thread);
    }
}
```

### Action #3: Tests basiques (1-2h)

**Test 1**: 2 threads sur 2 CPUs
```rust
#[test]
fn test_smp_scheduler_basic() {
    // Créer 2 threads
    let t1 = Thread::new_kernel(task1);
    let t2 = Thread::new_kernel(task2);
    
    // Assigner à CPUs différents
    SCHEDULER.add_thread_to_cpu(t1, 0);
    SCHEDULER.add_thread_to_cpu(t2, 1);
    
    // Vérifier exécution parallèle
    sleep(100);
    assert!(both_tasks_completed());
}
```

---

## 📝 Checklist Phase 3

### Setup Initial
- [ ] Créer branche `feature/smp-scheduler`
- [ ] Backup code actuel
- [ ] Setup environnement test (4 CPUs)

### Implémentation
- [ ] PerCpuScheduler structure
- [ ] Integration avec SMP_SYSTEM
- [ ] Load balancing basique
- [ ] CPU affinity API
- [ ] Work stealing
- [ ] Logging lock-free
- [ ] Interrupts sur APs
- [ ] TLB shootdown

### Tests
- [ ] Test 2 threads / 2 CPUs
- [ ] Test 8 threads / 4 CPUs
- [ ] Test load balancing
- [ ] Test work stealing
- [ ] Test migration
- [ ] Test TLB shootdown
- [ ] Benchmarks performance

### Documentation
- [ ] API documentation
- [ ] Architecture diagram
- [ ] Performance report
- [ ] PHASE_3_COMPLETE.md

---

## 🎯 Critères de Validation Phase 3

**Must Have** (Requis):
- ✅ 4 CPUs exécutent threads simultanément
- ✅ Load balancing fonctionne
- ✅ Pas de deadlocks
- ✅ Pas de corruption mémoire
- ✅ Tests passent sur 100 runs

**Should Have** (Souhaité):
- ✅ Efficacité > 90% @ 4 CPUs
- ✅ Latency variance < 20%
- ✅ Logging complet depuis tous CPUs
- ✅ TLB shootdown < 10μs

**Nice to Have** (Bonus):
- CPU hotplug (add/remove CPU runtime)
- NUMA awareness
- Cache-line optimization
- Real-time scheduling per-CPU

---

## 📚 Ressources

### Code de référence
- Linux: `kernel/sched/core.c`, `kernel/sched/fair.c`
- FreeBSD: `sys/kern/sched_ule.c`
- Zircon: `kernel/kernel/sched.cpp`

### Documentation
- [OSDev - SMP Scheduling](https://wiki.osdev.org/SMP)
- [Linux CFS](https://docs.kernel.org/scheduler/sched-design-CFS.html)
- Intel SDM Vol. 3A Chapter 8

---

**Phase 2 accomplie! Cap sur Phase 3! 🚀**
