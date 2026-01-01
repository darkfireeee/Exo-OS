# 🚀 Phase 2 - Plan d'Action SMP Multi-Core
## Migration de Single-Core vers Multi-Core

**Date de début:** 27 décembre 2025  
**Objectif:** Activer support SMP (Symmetric Multi-Processing)  
**Durée estimée:** 2-3 semaines  
**Prérequis:** ✅ Phases 0-1 complètes et validées

---

## 🎯 OBJECTIFS PHASE 2

### Objectifs Principaux
1. **ACPI Parsing** - Détecter CPUs disponibles via MADT table
2. **AP Bootstrap** - Démarrer Application Processors (CPUs secondaires)
3. **Per-CPU Structures** - Données et queues isolées par CPU
4. **Load Balancing** - Distribution intelligente des threads
5. **IPI Communication** - Inter-Processor Interrupts
6. **TLB Shootdown** - Synchronisation invalidation TLB

### Critères de Validation
- [ ] Boot 4 CPUs (QEMU `-smp 4`)
- [ ] Scheduler distribue threads sur tous les cores
- [ ] Context switches simultanés sur chaque CPU
- [ ] Load balancing fonctionnel (work stealing)
- [ ] IPI pour migration threads
- [ ] TLB shootdown correct (pas de corruption mémoire)
- [ ] Performance: overhead SMP < 10%

---

## 📂 ÉTAT ACTUEL DU CODE SMP

### Fichiers Existants (35-50% complets)

#### `kernel/src/arch/x86_64/smp/` (~35%)
**Fichiers:**
- `mod.rs` - Module principal SMP
- `ap_boot.rs` - AP bootstrap code (structure présente)
- `ipi.rs` - Inter-Processor Interrupts (stubs)
- `percpu.rs` - Per-CPU data structures

**Status:**
- ⚠️ Structures définies mais code non activé
- ⚠️ AP trampoline 16-bit non écrit
- ⚠️ ACPI parsing minimal

#### `kernel/src/scheduler/core/per_cpu.rs` (~40%)
**Structure:**
```rust
pub struct PerCpuData {
    pub cpu_id: u32,
    pub apic_id: u32,
    pub scheduler_queue: SchedulerQueue,  // Queue locale
    pub idle_thread: Option<Arc<Thread>>,
    pub current_thread: Option<Arc<Thread>>,
    pub stats: CpuStatistics,
}
```

**Status:**
- ✅ Structure bien définie
- ⚠️ Pas encore intégrée au scheduler
- ⚠️ Initialisation manquante

#### `kernel/src/scheduler/load_balance/` (~30%)
**Fichiers:**
- `mod.rs` - Module principal
- `work_stealing.rs` - Algorithme work stealing (écrit)
- `migration.rs` - Thread migration (structure)

**Status:**
- ✅ Algorithmes écrits théoriquement
- ⚠️ Pas testé
- ⚠️ Métriques load pas collectées

---

## 🗺️ ROADMAP PHASE 2

### Semaine 1: ACPI + AP Bootstrap

#### Jour 1-2: ACPI Parsing
**Objectif:** Détecter CPUs via MADT (Multiple APIC Description Table)

**Tâches:**
1. [ ] Parser RSDP (Root System Description Pointer)
2. [ ] Trouver MADT table
3. [ ] Extraire liste LAPIC (Local APIC) entries
4. [ ] Stocker CPU count et APIC IDs
5. [ ] Validation: afficher "Found X CPUs"

**Fichiers à créer/modifier:**
- `kernel/src/arch/x86_64/acpi/mod.rs` (nouveau)
- `kernel/src/arch/x86_64/acpi/madt.rs` (nouveau)
- `kernel/src/arch/x86_64/acpi/rsdp.rs` (nouveau)

**Code estimé:** ~300 lignes

**Ressources:**
- ACPI Specification v6.3: https://uefi.org/specs/ACPI/6.3/
- OSDev ACPI: https://wiki.osdev.org/ACPI

#### Jour 3-4: APIC Initialization
**Objectif:** Configurer Local APIC et I/O APIC

**Tâches:**
1. [ ] Mapper LAPIC registers (0xFEE00000)
2. [ ] Enable LAPIC via MSR
3. [ ] Configure LAPIC timer (remplacer PIT)
4. [ ] Configurer I/O APIC pour IRQs
5. [ ] Tester timer interrupt sur BSP (Bootstrap Processor)

**Fichiers:**
- `kernel/src/arch/x86_64/apic/lapic.rs` (nouveau)
- `kernel/src/arch/x86_64/apic/ioapic.rs` (nouveau)
- Modifier `kernel/src/arch/x86_64/pit.rs` (migration vers APIC)

**Code estimé:** ~400 lignes

**Attention:** Migration PIT → APIC timer peut casser scheduler temporairement

#### Jour 5-7: AP Bootstrap (Trampoline)
**Objectif:** Démarrer CPUs secondaires en mode 64-bit

**Tâches:**
1. [ ] Écrire AP trampoline (16-bit real mode)
2. [ ] Transition 16-bit → 32-bit protected mode
3. [ ] Transition 32-bit → 64-bit long mode
4. [ ] Charger GDT/IDT per-CPU
5. [ ] Envoyer INIT-SIPI-SIPI IPI sequence
6. [ ] Validation: tous les CPUs atteignent `ap_main()`

**Fichiers:**
- `kernel/src/arch/x86_64/smp/ap_trampoline.s` (nouveau, ASM)
- `kernel/src/arch/x86_64/smp/ap_boot.rs` (compléter)
- `kernel/src/arch/x86_64/smp/startup.rs` (nouveau)

**Code estimé:** ~500 lignes (200 ASM + 300 Rust)

**Défis:**
- Trampoline doit être en dessous de 1MB (real mode addressing)
- Synchronisation BSP/APs (spinlocks temporaires)
- Chaque AP doit avoir son propre stack

### Semaine 2: Per-CPU + Scheduler

#### Jour 8-10: Per-CPU Structures
**Objectif:** Isoler données critiques par CPU

**Tâches:**
1. [ ] Allouer PerCpuData pour chaque CPU
2. [ ] GDT per-CPU (GSbase MSR)
3. [ ] Scheduler queues per-CPU
4. [ ] Idle thread per-CPU
5. [ ] Stats per-CPU (lock-free counters)

**Fichiers:**
- `kernel/src/scheduler/core/per_cpu.rs` (compléter)
- `kernel/src/arch/x86_64/gdt.rs` (per-CPU support)
- `kernel/src/scheduler/core/scheduler.rs` (refactor multi-queue)

**Code estimé:** ~600 lignes

**Architecture:**
```
CPU 0: [Hot Queue] [Normal Queue] [Cold Queue] [Idle Thread]
CPU 1: [Hot Queue] [Normal Queue] [Cold Queue] [Idle Thread]
CPU 2: [Hot Queue] [Normal Queue] [Cold Queue] [Idle Thread]
CPU 3: [Hot Queue] [Normal Queue] [Cold Queue] [Idle Thread]
```

#### Jour 11-12: Scheduler Multi-Core
**Objectif:** Distribuer threads sur tous les cores

**Tâches:**
1. [ ] `add_thread()` → choisir CPU cible (load balancing)
2. [ ] `pick_next()` → chercher dans queue locale
3. [ ] Fallback global queue si queue locale vide
4. [ ] CPU affinity support (soft/hard)
5. [ ] Thread migration API

**Fichiers:**
- `kernel/src/scheduler/core/scheduler.rs` (refactor majeur)
- `kernel/src/scheduler/core/multi_core.rs` (nouveau)
- `kernel/src/scheduler/thread/thread.rs` (champ `cpu_affinity`)

**Code estimé:** ~400 lignes

**Métriques à suivre:**
- Threads per CPU
- Queue depths
- Migrations count
- Load imbalance

#### Jour 13-14: IPI (Inter-Processor Interrupts)
**Objectif:** Communication entre CPUs

**Tâches:**
1. [ ] IPI send function (write to LAPIC ICR)
2. [ ] IPI receive handler (vector 240-250)
3. [ ] TLB shootdown IPI (invalider TLB distant)
4. [ ] Reschedule IPI (forcer context switch)
5. [ ] Function call IPI (exécuter fonction sur autre CPU)

**Fichiers:**
- `kernel/src/arch/x86_64/smp/ipi.rs` (compléter)
- `kernel/src/arch/x86_64/handlers.rs` (IPI handlers)

**Code estimé:** ~300 lignes

**Use cases:**
- TLB shootdown après `munmap()`
- Thread migration urgente
- System shutdown coordination

### Semaine 3: Load Balancing + Tests

#### Jour 15-17: Load Balancing
**Objectif:** Équilibrer charge entre CPUs

**Tâches:**
1. [ ] Collecter métriques load per-CPU (EMA-based)
2. [ ] Work stealing: CPU idle vole threads d'autres CPUs
3. [ ] Push migration: CPU surchargé pousse threads
4. [ ] Affinity respectée (soft affinity = préférence)
5. [ ] Validation: distribution uniforme

**Fichiers:**
- `kernel/src/scheduler/load_balance/work_stealing.rs` (activer)
- `kernel/src/scheduler/load_balance/migration.rs` (activer)
- `kernel/src/scheduler/load_balance/metrics.rs` (nouveau)

**Code estimé:** ~500 lignes

**Algorithme work stealing:**
```
if (local_queue.is_empty()) {
    for victim_cpu in random_order(other_cpus) {
        if let Some(thread) = victim_cpu.queue.steal_half() {
            local_queue.push(thread);
            break;
        }
    }
}
```

#### Jour 18-19: TLB Shootdown
**Objectif:** Synchroniser invalidation TLB multi-core

**Tâches:**
1. [ ] `invlpg_all_cpus(addr)` - Invalider page sur tous CPUs
2. [ ] Envoyer TLB shootdown IPI
3. [ ] Attendre ACK de tous les CPUs
4. [ ] Support PCID (invalider PCID spécifique)
5. [ ] Validation: pas de corruption mémoire

**Fichiers:**
- `kernel/src/arch/x86_64/memory/tlb.rs` (nouveau)
- `kernel/src/arch/x86_64/pcid.rs` (étendre pour SMP)

**Code estimé:** ~250 lignes

**Séquence:**
```
1. CPU 0: munmap(addr) → doit invalider TLB sur tous CPUs
2. CPU 0: send_ipi_tlb_shootdown(addr) → tous les CPUs
3. CPU 1,2,3: reçoivent IPI → invlpg(addr)
4. CPU 1,2,3: ACK
5. CPU 0: wait_for_ack() → continue
```

#### Jour 20-21: Tests et Validation
**Objectif:** Valider SMP complet

**Tests à créer:**
1. [ ] `test_smp_boot()` - Tous les CPUs démarrent
2. [ ] `test_scheduler_distribution()` - Threads sur tous CPUs
3. [ ] `test_work_stealing()` - Idle CPU vole threads
4. [ ] `test_ipi_communication()` - IPI fonctionnel
5. [ ] `test_tlb_shootdown()` - Pas de corruption
6. [ ] `test_concurrent_context_switch()` - Simultané sur tous CPUs

**Benchmarks:**
1. [ ] `bench_smp_overhead()` - Overhead SMP vs single-core
2. [ ] `bench_context_switch_multicore()` - Performance multi-core
3. [ ] `bench_load_balancing()` - Temps de distribution

**Fichier:**
- `kernel/src/tests/smp_tests.rs` (nouveau)

**Code estimé:** ~400 lignes

**Critères de succès:**
- ✅ 4 CPUs bootent en mode 64-bit
- ✅ Scheduler distribue uniformément
- ✅ Work stealing actif (métriques)
- ✅ IPI latence < 500 ns
- ✅ TLB shootdown < 2 µs
- ✅ Overhead SMP < 10%
- ✅ Pas de deadlocks (48h stress test)

---

## 🔧 CONFIGURATION QEMU

### Testing SMP
```bash
# 4 CPUs
qemu-system-x86_64 -smp 4 -m 512M -cdrom build/exo_os.iso

# 8 CPUs (stress test)
qemu-system-x86_64 -smp 8 -m 1024M -cdrom build/exo_os.iso

# Debug APIC
qemu-system-x86_64 -smp 4 -d int,cpu_reset -D qemu_smp.log
```

### Expected Output
```
[ACPI] Found 4 CPUs in MADT
[APIC] BSP LAPIC ID: 0
[APIC] Initializing Local APIC...
[SMP] Starting AP 1 (APIC ID: 1)
[SMP] Starting AP 2 (APIC ID: 2)
[SMP] Starting AP 3 (APIC ID: 3)
[SMP] All 4 CPUs online
[SCHED] Per-CPU queues initialized
[SCHED] Load balancing active
```

---

## ⚠️ RISQUES ET MITIGATIONS

### Risques Majeurs

#### 1. Race Conditions
**Risque:** Accès concurrent aux structures partagées
**Mitigation:**
- Per-CPU data autant que possible
- Atomic operations (CAS, fetch_add)
- Spinlocks seulement si nécessaire (minimal holding time)
- Seqlocks pour read-heavy workloads

#### 2. Deadlocks
**Risque:** Lock ordering violations
**Mitigation:**
- Lock hierarchy stricte (documenter ordre)
- Trylock avec timeout
- Deadlock detector (debug builds)
- Stress tests 48h+

#### 3. TLB Shootdown Bugs
**Risque:** Corruption mémoire si TLB pas invalidé
**Mitigation:**
- Tests exhaustifs avec KASAN (Kernel Address Sanitizer)
- Vérifier chaque `munmap()` invalide TLB
- Logs détaillés en debug mode
- Unit tests avec patterns connus

#### 4. Performance Regression
**Risque:** Overhead SMP > gains multi-core
**Mitigation:**
- Benchmarks avant/après chaque changement
- Profiling avec `perf` (si KVM activé)
- Lock-free autant que possible
- Batch operations (réduire synchronization)

#### 5. AP Boot Failure
**Risque:** CPUs secondaires ne bootent pas
**Mitigation:**
- Logs détaillés dans trampoline
- Retry logic (INIT-SIPI-SIPI peut échouer)
- Timeout et fallback single-core
- Test sur hardware réel (pas seulement QEMU)

---

## 📊 MÉTRIQUES DE SUCCÈS

### Performance Targets

| Métrique | Target | Baseline (Single-Core) |
|----------|--------|------------------------|
| Boot time (4 CPUs) | < 3s | 2s |
| Context switch overhead | < 5% | 228 cycles |
| IPI latency | < 500ns | N/A |
| TLB shootdown | < 2µs | N/A |
| Load balancing time | < 100µs | N/A |
| Work stealing latency | < 50µs | N/A |

### Scalability Targets

| CPUs | Throughput | Efficiency |
|------|------------|------------|
| 1 | 100% (baseline) | 100% |
| 2 | 190% | 95% |
| 4 | 360% | 90% |
| 8 | 680% | 85% |

**Efficiency = Throughput / (CPUs × 100%)**

---

## 📚 RESSOURCES

### Documentation
- [OSDev SMP](https://wiki.osdev.org/Symmetric_Multiprocessing)
- [OSDev APIC](https://wiki.osdev.org/APIC)
- [Intel SDM Vol 3A Chapter 10](https://software.intel.com/content/www/us/en/develop/articles/intel-sdm.html)
- [ACPI Spec 6.3](https://uefi.org/specs/ACPI/6.3/)

### Code Références
- Linux: `arch/x86/kernel/smpboot.c`
- xv6: `kernel/mp.c`
- SerenityOS: `Kernel/Arch/x86_64/SMP.cpp`

### Tools
- QEMU: `-smp N -d int,cpu_reset`
- GDB: `info threads`, `thread N`
- KVM: `perf record -e sched:*`

---

## ✅ CHECKLIST AVANT DE COMMENCER

- [x] Phases 0-1 validées (documentation créée)
- [x] Context switch optimal (228 cycles)
- [x] QEMU installé avec support SMP
- [ ] Lire Intel SDM Vol 3A Chapter 10 (APIC)
- [ ] Lire ACPI Spec sections 5.2.12 (MADT)
- [ ] Setup debugging: `qemu -s -S` + GDB
- [ ] Backup code actuel (git commit)
- [ ] Plan B: feature flags pour désactiver SMP si bloqué

---

## 🎯 PROCHAINE ÉTAPE IMMÉDIATE

**Commencer par:** ACPI Parsing (Jour 1-2)

**Fichier à créer:** `kernel/src/arch/x86_64/acpi/mod.rs`

**Premier objectif:** Afficher `[ACPI] Found X CPUs`

**Commande:**
```bash
# Créer structure module ACPI
mkdir -p kernel/src/arch/x86_64/acpi
touch kernel/src/arch/x86_64/acpi/mod.rs
touch kernel/src/arch/x86_64/acpi/rsdp.rs
touch kernel/src/arch/x86_64/acpi/madt.rs
```

---

**Date de création:** 27 décembre 2025  
**Dernière mise à jour:** 27 décembre 2025  
**Status:** ✅ Plan prêt, en attente de démarrage Phase 2
