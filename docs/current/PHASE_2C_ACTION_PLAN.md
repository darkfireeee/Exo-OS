# Phase 2c - Plan d'Action Critique
**Date:** 2026-01-01  
**Status:** 🚀 READY TO START  
**Priority:** CRITICAL pour validation Phase 2b

---

## 🎯 Objectif Phase 2c

**Valider et optimiser le scheduler SMP** avant de passer à Phase 3 (IPC Integration)

### Pourquoi c'est critique:
1. **Tests fonctionnels** → Valider que le SMP scheduler marche réellement
2. **Optimisations** → Atteindre les targets de performance
3. **Cleanup TODOs** → Éliminer les 15 TODOs critiques scheduler
4. **Stabilité** → Base solide pour IPC-SMP integration

---

## 📊 Tests - État Actuel

### Tests Créés ✅
- `smp_tests.rs` - 6 tests fonctionnels
- `smp_bench.rs` - 4 benchmarks performance
- Auto-exécution au boot (Phase 2.8-2.9)

### Problème: Pas d'Exécution ❌
**Environnement actuel** (devcontainer):
- ❌ QEMU TCG - SMP limité, APs ne démarrent pas toujours
- ❌ Bochs - Mode interactif, pas adapté CI/CD
- ❌ KVM - Non disponible (nested virtualization disabled)

**Solution:** Tests nécessitent **HARDWARE RÉEL** ou **QEMU avec KVM**

---

## 🚀 Plan Phase 2c (67h - 4-5 semaines)

### WEEK 1: Tests Unitaires (Sans Hardware) ✅ PEUT COMMENCER
**Focus:** Tester la logique du scheduler isolément

#### 1.1 Tests per_cpu_queue (8h)
```rust
// Tests déjà créés mais pas exécutés - améliorer:
#[test]
fn test_percpu_queue_thread_safety() {
    // Tester concurrent access simulation
}

#[test]
fn test_work_stealing_fairness() {
    // Vérifier distribution équitable
}

#[test]
fn test_statistics_accuracy() {
    // Valider counters
}
```
**Deliverable:** Tests unitaires Rust standard (pas besoin de QEMU)

#### 1.2 Benchmark Micro (6h)
```rust
// Mesurer les opérations en isolation
#[bench]
fn bench_enqueue_single_thread() { ... }

#[bench]
fn bench_dequeue_single_thread() { ... }

#[bench]
fn bench_steal_contention() { ... }
```
**Deliverable:** Benchmarks `cargo bench`

### WEEK 2: Cleanup TODOs Critiques ✅ PEUT COMMENCER
**Focus:** Éliminer les 15 TODOs scheduler

#### 2.1 Blocked Threads Management (8h)
- [ ] **scheduler.rs:596** - `// TODO: Add to blocked list`
- [ ] Implémenter `blocked_threads: HashMap<ThreadId, BlockReason>`
- [ ] Tests: block/unblock cycles
- [ ] Validation: Aucun thread perdu

#### 2.2 Thread Termination Cleanup (8h)
- [ ] **scheduler.rs:918** - `// TODO: Terminate thread properly`
- [ ] Cleanup: stack, FDs, memory maps
- [ ] Tests: Pas de leaks après 1000 terminaisons
- [ ] Validation: Valgrind-like checks

#### 2.3 FPU/SIMD State (10h)
- [ ] **switch/mod.rs:9** - FPU stubs
- [ ] Intégrer `arch::x86_64::fpu`
- [ ] Save/restore XSAVE area
- [ ] Tests: FPU state preservation cross context-switch

**Deliverable:** 15 TODOs → 0, tests unitaires PASS

### WEEK 3: IPC-Timer Integration ✅ PEUT COMMENCER
**Focus:** Préparer pour Phase 3

#### 3.1 Timer Subsystem (8h)
- [ ] **futex.rs:250** - Timer integration
- [ ] **endpoint.rs:302,380** - Timeouts
- [ ] Utiliser `time::Timer` API
- [ ] Tests: Timeout functionality

#### 3.2 Priority Inheritance (10h)
- [ ] **futex.rs:435** - Priority inheritance
- [ ] Algorithme: Boost priorité sur lock
- [ ] Tests: Éviter priority inversion
- [ ] Validation: Scénarios classiques

**Deliverable:** IPC prêt pour integration SMP

### WEEK 4-5: Tests Hardware (CRITIQUE - Nécessite hardware)
**Focus:** Validation réelle SMP

#### 4.1 Préparation Test Environment
- [ ] Documentation: Hardware requirements
- [ ] Script: Automated testing suite
- [ ] CI/CD: GitHub Actions avec KVM?
- [ ] Fallback: Instructions tests manuels

#### 4.2 Tests SMP Réels (Nécessite hardware)
```bash
# Sur machine réelle avec 4+ CPUs
./run_smp_tests_real_hardware.sh

Expected:
✅ test_percpu_queues_init - PASS
✅ test_local_enqueue_dequeue - PASS
✅ test_work_stealing - PASS (critical!)
✅ test_percpu_stats - PASS
✅ test_idle_threads - PASS
✅ test_context_switch_count - PASS

Benchmarks:
✅ cpu_id: <10 cycles
✅ enqueue: <100 cycles
✅ dequeue: <100 cycles
✅ work_stealing: <5000 cycles
```

#### 4.3 Stress Tests (Nécessite hardware)
- [ ] 10,000 threads créés/détruits
- [ ] Work stealing sous charge
- [ ] Fairness multi-CPU
- [ ] Memory leaks check

**Deliverable:** Rapport validation hardware complet

### WEEK 5: Optimizations (Basé sur résultats hardware)
- [ ] Tune work_stealing threshold
- [ ] Optimize queue sizes
- [ ] CPU affinity hints
- [ ] Reduce lock contention

**Deliverable:** Performance targets atteints

---

## ✅ Ce qu'on PEUT faire MAINTENANT (Sans Hardware)

### Priorité 1: Tests Unitaires (Week 1) - 14h
```bash
# Dans kernel/src/scheduler/tests/
cargo test --lib percpu_queue
cargo test --lib schedule_smp
cargo bench scheduler
```
**Pas besoin de QEMU/Bochs** - Tests Rust standards

### Priorité 2: Cleanup TODOs (Week 2) - 26h
- Blocked threads management
- Thread termination cleanup
- FPU/SIMD integration

**Validation:** Tests unitaires Rust

### Priorité 3: IPC-Timer (Week 3) - 18h
- Timer integration
- Priority inheritance

**Validation:** Tests unitaires + doc

**Total SANS hardware:** 58h (~4 semaines)

---

## ❌ Ce qu'on NE PEUT PAS faire (Sans Hardware)

### Tests SMP Réels
- Work stealing validation cross-CPU
- Performance benchmarks réels
- Stress tests multi-core
- Fairness validation

### Solution:
1. **Court terme:** Documenter tests manuels hardware
2. **Moyen terme:** CI/CD avec KVM (GitHub Actions)
3. **Long terme:** Lab hardware dédié

---

## 📋 Décision Point

### Option A: Commencer Phase 2c MAINTENANT ✅ RECOMMANDÉ
**Faire:**
- Week 1-3: Tests unitaires + Cleanup + IPC-Timer (58h)
- Documenter: Tests hardware requis
- Préparer: Scripts automation

**Avantages:**
- ✅ Progresse immédiatement
- ✅ Élimine TODOs critiques
- ✅ Prépare Phase 3
- ✅ Code validé par tests unitaires

**Risques:**
- ⚠️ Pas de validation hardware immédiate
- ⚠️ Peut découvrir bugs plus tard

### Option B: Attendre Hardware
**Faire:**
- Passer directement à Phase 3
- Revenir à Phase 2c avec hardware

**Avantages:**
- Pas de temps perdu sans validation

**Risques:**
- ❌ Phase 3 built sur base non-testée
- ❌ TODOs critiques persistent
- ❌ Bugs découverts plus tard = plus coûteux

---

## 🎯 RECOMMANDATION: Option A

**Commencer Phase 2c Week 1-3 IMMÉDIATEMENT**

### Justification:
1. **58h de travail utile** sans hardware requis
2. **Tests unitaires** valident logique même sans SMP réel
3. **Cleanup TODOs** nécessaire de toute façon
4. **Prépare Phase 3** proprement
5. **Documentation tests hardware** permet validation future

### Plan Immédiat (Prochaines 4 semaines):

```
Week 1 (14h):
  ✅ Tests unitaires percpu_queue
  ✅ Benchmarks micro
  ✅ Tests logique schedule_smp

Week 2 (26h):
  ✅ Blocked threads management
  ✅ Thread termination cleanup
  ✅ FPU/SIMD integration
  ✅ Éliminer 15 TODOs scheduler

Week 3 (18h):
  ✅ Timer integration
  ✅ Priority inheritance
  ✅ IPC-SMP prep

Week 4 (8h):
  ✅ Documentation tests hardware
  ✅ Scripts automation
  ✅ Plan validation future
  ✅ Release v0.7.0-alpha
```

**Total:** 66h → v0.7.0-alpha prêt pour validation hardware

---

## 📊 Métriques de Succès (Sans Hardware)

### Week 1-3:
- [ ] 15 TODOs scheduler → 0
- [ ] Tests unitaires: 20+ tests PASS
- [ ] Benchmarks: Baselines établis
- [ ] FPU/SIMD: Intégré et testé
- [ ] IPC-Timer: Fonctionnel en tests

### Documentation:
- [ ] Hardware test plan complet
- [ ] Automation scripts prêts
- [ ] Performance targets définis
- [ ] Validation checklist

### Release v0.7.0-alpha:
- ✅ Code clean (0 TODOs critiques)
- ✅ Tests unitaires PASS
- ⏳ Tests hardware: PENDING
- 📋 Ready for real hardware validation

---

## 🚀 Action Immédiate

**Commencer MAINTENANT avec Week 1:**

```bash
# 1. Créer les tests unitaires
cd kernel/src/scheduler/
mkdir -p tests
touch tests/percpu_queue_tests.rs
touch tests/schedule_smp_tests.rs
touch tests/benchmarks.rs

# 2. Implémenter
cargo test --lib
cargo bench

# 3. CI/CD
.github/workflows/tests.yml
```

**Estimated: 2h setup + 12h tests = 14h total**

---

**Status:** 📋 PLAN READY  
**Decision Needed:** Commencer Phase 2c maintenant?  
**Recommended:** ✅ OUI - Option A  
**Next:** Create test files et commencer Week 1

*Tests hardware viendront plus tard - le code sera prêt!*
