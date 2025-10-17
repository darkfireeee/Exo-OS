# 🎯 Plan de Test et d'Optimisation Exo-OS

## ✅ État Actuel (17 Octobre 2025)

### Compilation
- ✅ **Kernel compile sans erreurs** (0 erreurs, 42 warnings)
- ✅ **C code compile** (serial.c, pci.c)
- ✅ **Build time**: ~1 seconde (build incrémental)
- ⚠️ **Warnings**: Principalement variables non utilisées (acceptable pour MVP)

### Architecture
```
Exo-OS/
├── kernel/
│   ├── src/
│   │   ├── lib.rs              ✅ Point d'entrée
│   │   ├── arch/x86_64/        ✅ GDT, IDT, interrupts
│   │   ├── memory/             ⚠️ Stubs (frame_allocator, heap)
│   │   ├── scheduler/          ✅ Thread, Scheduler
│   │   ├── ipc/                ✅ Channels, Messages
│   │   ├── syscall/            ⚠️ Stub (dispatch)
│   │   ├── drivers/            ⚠️ Stub (block)
│   │   └── c_compat/           ✅ Serial, PCI
│   └── tests/
│       └── basic_boot.rs       ✅ Test de boot
├── linker.ld                   ✅ Script linker
├── x86_64-unknown-none.json    ✅ Target spec
├── test-qemu.ps1               ✅ Script de test
├── TESTING.md                  ✅ Guide de test
└── Makefile                    ✅ Commandes rapides
```

---

## 📋 Phase 1: VALIDATION (Avant Optimisation)

### 1.1 Boot Test ⏳ EN COURS

**Objectif**: Valider que le kernel peut démarrer

**Actions**:
```powershell
# Option A: Avec bootimage (recommandé)
cargo install bootimage
rustup component add llvm-tools-preview
cd kernel
cargo bootimage --run

# Option B: Avec script PowerShell
.\test-qemu.ps1
```

**Critères de succès**:
- [ ] QEMU démarre
- [ ] Serial output visible
- [ ] Message "Exo-OS Starting..." affiché
- [ ] Pas de panic/crash
- [ ] GDT/IDT chargées

**Estimation**: 30 minutes - 1 heure

---

### 1.2 Système de Base ⏳ À FAIRE

**Objectif**: Valider les sous-systèmes critiques

**Tests à implémenter**:

```rust
// tests/interrupts.rs
#[test_case]
fn test_breakpoint_interrupt() {
    // Déclencher un breakpoint
    x86_64::instructions::interrupts::int3();
    // Si on arrive ici, l'IDT fonctionne
}

// tests/memory.rs
#[test_case]
fn test_heap_allocation() {
    let vec = alloc::vec![1, 2, 3];
    assert_eq!(vec.len(), 3);
}

// tests/scheduler.rs
#[test_case]
fn test_thread_creation() {
    let id = scheduler::spawn(test_fn, Some("test"), None);
    assert!(id > 0);
}

// tests/ipc.rs
#[test_case]
fn test_channel_send_receive() {
    let channel = ipc::create_channel("test", 10)?;
    // Test send/receive
}
```

**Critères de succès**:
- [ ] Interrupts fonctionnent
- [ ] Heap allocation OK
- [ ] Thread creation OK
- [ ] IPC channels OK

**Estimation**: 2-3 heures

---

### 1.3 Instrumentation ⏳ À FAIRE

**Objectif**: Établir baseline de performance

**Ajouter des compteurs**:

```rust
// kernel/src/instrumentation.rs
pub struct Metrics {
    pub ipc_calls: AtomicU64,
    pub context_switches: AtomicU64,
    pub syscalls: AtomicU64,
    pub interrupts: AtomicU64,
}

pub static METRICS: Metrics = Metrics::new();

// Utilisation
pub fn measure_ipc_latency() -> u64 {
    let start = rdtsc();
    // IPC call
    let end = rdtsc();
    end - start
}
```

**Critères de succès**:
- [ ] rdtsc() fonctionne
- [ ] Compteurs incrémentés
- [ ] Latences mesurables

**Estimation**: 1-2 heures

---

## 📊 Phase 2: BASELINE (Mesures Initiales)

### 2.1 Établir les Métriques de Base

**Mesurer AVANT optimisation**:

| Métrique | Comment Mesurer | Baseline Attendue | Objectif Final |
|----------|-----------------|-------------------|----------------|
| **IPC Latency** | rdtsc avant/après send() | ~5-10 µs ? | < 500 ns |
| **Context Switch** | rdtsc dans scheduler | ~10-20 µs ? | < 1 µs |
| **Syscall** | getpid() x 1M / temps | ~500K/sec ? | > 5M/sec |
| **Boot Time** | Timer depuis reset | ~2-5 sec ? | < 500 ms |
| **Thread Spawn** | Créer 1000 threads | ~1-5 ms ? | < 100 µs/thread |

**Script de benchmark**:

```rust
// kernel/benches/baseline.rs
#[bench]
fn bench_ipc_latency() {
    let channel = create_channel("bench", 1).unwrap();
    let msg = Message::fast([0u8; 64]);
    
    let iterations = 1_000_000;
    let start = rdtsc();
    
    for _ in 0..iterations {
        channel.send(msg).unwrap();
    }
    
    let end = rdtsc();
    let avg_cycles = (end - start) / iterations;
    
    println!("IPC: {} cycles/call", avg_cycles);
}
```

**Critères de succès**:
- [ ] Tous les benchmarks s'exécutent
- [ ] Résultats cohérents (variance < 10%)
- [ ] Baseline documentée

**Estimation**: 2-3 heures

---

## 🚀 Phase 3: OPTIMISATION (Après Validation)

### 3.1 IPC: < 500ns (Objectif: ~300ns)

**Stratégies**:
1. **Fast Path**: Cas optimisé sans lock
   ```rust
   // Si queue vide et un seul producteur
   if likely(queue.is_fast_path()) {
       queue.push_unchecked(msg);  // Pas de lock
   }
   ```

2. **Zero-Copy**: Éviter les copies
   ```rust
   // Message dans shared memory
   struct FastMessage {
       ptr: *const u8,  // Pointeur vers data
       len: usize,
   }
   ```

3. **SPSC Queue**: Lock-free pour cas 1-1
   ```rust
   use crossbeam_queue::ArrayQueue;
   ```

**Métriques cibles**:
- Baseline: ~5-10 µs
- Après fast-path: ~1-2 µs
- Après zero-copy: ~500-700 ns
- Après SPSC: ~300-400 ns ✅

---

### 3.2 Context Switch: < 1µs (Objectif: ~700ns)

**Stratégies**:
1. **Lazy FPU**: Ne sauver que si utilisé
   ```asm
   ; Ne sauver FPU que si TS flag set
   test cr0, 0x8
   jz skip_fpu_save
   ```

2. **Minimal State**: Sauver le minimum
   ```rust
   // Seulement: RIP, RSP, RFLAGS + registres callee-saved
   struct MinimalContext {
       rip: u64,
       rsp: u64,
       rbp: u64,
       rbx: u64,
       r12: u64, r13: u64, r14: u64, r15: u64,
   }
   ```

3. **Per-CPU Stack**: Éviter cache misses
   ```rust
   #[thread_local]
   static SCHEDULER_STACK: [u8; 4096];
   ```

**Métriques cibles**:
- Baseline: ~10-20 µs
- Après lazy FPU: ~3-5 µs
- Après minimal state: ~1-2 µs
- Après per-CPU: ~700-900 ns ✅

---

### 3.3 Syscall: > 5M/sec (Objectif: ~150ns/call)

**Stratégies**:
1. **SYSCALL/SYSRET**: Au lieu de INT 0x80
   ```asm
   syscall  ; Plus rapide que int 0x80
   ```

2. **Table de Jump**: Dispatch direct
   ```rust
   static SYSCALL_TABLE: [fn(); 256] = [/* ... */];
   
   #[naked]
   unsafe extern "C" fn syscall_handler() {
       asm!("
           lea rax, [rip + SYSCALL_TABLE]
           call [rax + rdi*8]
           sysretq
       ");
   }
   ```

3. **Validation Minimale**: Fast path sans checks
   ```rust
   if likely(is_simple_syscall(num)) {
       return fast_syscall(num, args);  // Inline
   }
   ```

**Métriques cibles**:
- Baseline: ~500K/sec (2 µs/call)
- Après SYSCALL: ~2M/sec (500ns)
- Après jump table: ~4M/sec (250ns)
- Après fast path: ~6M/sec (150ns) ✅

---

### 3.4 Boot Time: < 500ms (Objectif: ~300ms)

**Stratégies**:
1. **Parallel Init**: Initialiser en parallèle
   ```rust
   // Démarrer sur tous les CPUs simultanément
   for cpu in 1..num_cpus {
       start_ap(cpu, init_secondary);
   }
   ```

2. **Lazy Driver Loading**: Charger à la demande
   ```rust
   // Ne charger que les drivers critiques au boot
   drivers::load_critical();  // Serial, timer
   // Autres drivers: lazy load
   ```

3. **Minimal Boot Services**: Réduire au strict nécessaire
   ```rust
   fn boot_kernel() {
       serial_init();      // 1ms
       gdt_init();         // <1ms
       idt_init();         // <1ms
       memory_init();      // 50ms
       scheduler_init();   // 10ms
       // Total: ~65ms base
   }
   ```

**Métriques cibles**:
- Baseline: ~2-5 sec
- Après parallel: ~1 sec
- Après lazy loading: ~500ms
- Après minimal: ~300ms ✅

---

## 📈 Phase 4: VALIDATION FINALE

### 4.1 Tests de Performance

**Suite complète de benchmarks**:
```powershell
cargo bench --target ../x86_64-unknown-none.json
```

**Résultats attendus**:

| Métrique | Baseline | Optimisé | Objectif | ✅ |
|----------|----------|----------|----------|---|
| IPC Latency | 5-10 µs | 300-400 ns | < 500 ns | ✅ |
| Context Switch | 10-20 µs | 700-900 ns | < 1 µs | ✅ |
| Syscall | 500K/sec | 6M/sec | > 5M/sec | ✅ |
| Boot Time | 2-5 sec | 300ms | < 500ms | ✅ |

---

### 4.2 Tests de Stabilité

**Stress tests**:
```rust
// Créer 10000 threads
for i in 0..10000 {
    spawn(worker_fn, None, None);
}

// 1 million d'IPC calls
for _ in 0..1_000_000 {
    channel.send(msg)?;
}

// Boot/reboot 100 fois
for _ in 0..100 {
    reboot();
}
```

**Critères**:
- [ ] Pas de panic
- [ ] Pas de memory leak
- [ ] Performance stable

---

## 🎯 TIMELINE ESTIMÉE

| Phase | Durée | Description |
|-------|-------|-------------|
| **1. Validation** | 1-2 jours | Boot + tests de base |
| **2. Baseline** | 1 jour | Mesures initiales |
| **3. Optimisation** | 1-2 semaines | IPC, CS, Syscall, Boot |
| **4. Validation Finale** | 1-2 jours | Benchmarks + stabilité |
| **TOTAL** | **2-3 semaines** | MVP → Production Ready |

---

## ▶️ PROCHAINE ÉTAPE IMMÉDIATE

### Action: Tester le Boot

```powershell
# 1. Installer bootimage
cargo install bootimage
rustup component add llvm-tools-preview

# 2. Lancer le test
cd kernel
cargo bootimage --run

# Ou utiliser le script
cd ..
.\test-qemu.ps1
```

**Résultat attendu**:
```
Exo-OS Starting...
[ARCH] Initialisation de l'architecture x86_64...
[ARCH] Architecture x86_64 initialisée avec succès.
[SCHEDULER] Initialized for 4 CPUs.
...
```

---

**🎉 Une fois le boot validé, on pourra commencer l'optimisation !**
