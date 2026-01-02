# Phase 2c Week 2 - Cleanup 15 TODOs Scheduler

**Date**: 2026-01-01  
**Duration**: 26h planifiées  
**Status**: 🔜 STARTING

---

## 🎯 Objectif

Nettoyer les 15 TODOs critiques du scheduler pour stabilisation avant Phase 3.

**Catégories**:
1. **FPU/SIMD State Management** (10h) - Lazy FPU switching
2. **Blocked Threads Management** (8h) - Wait queues, condition variables  
3. **Thread Termination Cleanup** (8h) - Zombie handling, resource cleanup

---

## 📋 TODO List (15 items)

### Groupe A: FPU/SIMD Integration (10h)

#### ✅ Infrastructure Existante
Le fichier [kernel/src/arch/x86_64/fpu.rs](kernel/src/arch/x86_64/fpu.rs) contient déjà:
- Structure `FpuState` (512 bytes, 16-byte aligned)
- `fpu::init()` - Configuration CR0/CR4
- `fpu::save()` - FXSAVE
- `fpu::restore()` - FXRSTOR
- `fpu::set_task_switched()` - Set CR0.TS
- `fpu::clear_task_switched()` - CLTS
- `fpu::init_state()` - Initialize clean state

#### 🔴 TODO #1-7: FPU State Management

**TODO #1**: Intégrer FPU state dans Thread struct (1h)
```rust
// kernel/src/scheduler/thread/mod.rs
pub struct Thread {
    // ... existing fields
    fpu_state: FpuState,        // 512 bytes FPU/SSE state
    fpu_used: bool,             // Has thread used FPU?
}
```

**TODO #2**: Call set_task_switched() dans context switch (30min)
```rust
// kernel/src/scheduler/core/scheduler.rs - schedule()
// After context switch:
crate::arch::x86_64::fpu::set_task_switched();
```

**TODO #3**: Implémenter #NM handler (2h)
```rust
// kernel/src/arch/x86_64/interrupts/mod.rs
extern "x86-interrupt" fn device_not_available_handler(frame: InterruptStackFrame) {
    let current_tid = SCHEDULER.current_thread_id().unwrap();
    SCHEDULER.with_current_thread(|thread| {
        unsafe {
            fpu::handle_device_not_available(current_tid, &mut thread.fpu_state);
        }
        thread.set_fpu_used(true);
    });
}
```

**TODO #4**: Enregistrer #NM handler dans IDT (30min)
```rust
// kernel/src/arch/x86_64/idt.rs
idt[7].set_handler_fn(device_not_available_handler);
```

**TODO #5**: Lazy FPU save dans context switch (2h)
```rust
// Seulement save si fpu_used == true
if current_thread.fpu_used() {
    unsafe { fpu::save(&mut current_thread.fpu_state); }
}
```

**TODO #6**: FPU state init pour nouveaux threads (1h)
```rust
// kernel/src/scheduler/thread/mod.rs - Thread::new*()
let mut thread = Thread { /* ... */, fpu_used: false };
unsafe { fpu::init_state(&mut thread.fpu_state); }
```

**TODO #7**: Tests FPU (3h)
- Test: Thread sans FPU (pas de #NM)
- Test: Thread avec FPU (déclenche #NM, puis pas)
- Test: Multi-threads FPU (ownership tracking)

---

### Groupe B: Blocked Threads Management (8h)

#### ✅ Infrastructure Existante
- `scheduler::block_current()` - Bloque thread actuel
- `scheduler::unblock_thread(tid)` - Débloque par TID
- `blocked_threads: Mutex<BTreeMap>` - Registry blocked threads
- `sync::wait_queue::WaitQueue` - Wait queue basique
- `ipc::core::wait_queue::WaitQueue` - Wait queue IPC

#### 🔴 TODO #8-10: Améliorer Wait Queues

**TODO #8**: Condition Variables (3h)
```rust
// kernel/src/sync/condvar.rs
pub struct CondVar {
    wait_queue: WaitQueue,
}

impl CondVar {
    pub fn wait<T>(&self, mutex: &Mutex<T>) {
        // Unlock mutex, block, relock mutex
        drop(mutex.lock());
        self.wait_queue.wait();
        let _guard = mutex.lock();
    }
    
    pub fn notify_one(&self) {
        self.wait_queue.notify_one();
    }
    
    pub fn notify_all(&self) {
        self.wait_queue.notify_all();
    }
}
```

**TODO #9**: Timeout Support (3h)
```rust
// Améliorer wait_timeout() existant
pub fn wait_timeout(&self, timeout: Duration) -> WaitResult {
    // Déjà implémenté dans sync/wait_queue.rs
    // Vérifier intégration avec timer subsystem
}
```

**TODO #10**: Broadcast Wake (2h)
```rust
// Améliorer notify_all()
pub fn notify_all(&self) {
    // Déjà implémenté
    // Vérifier performance avec beaucoup de waiters
}
```

---

### Groupe C: Thread Termination Cleanup (8h)

#### ✅ Infrastructure Existante
- `ThreadState::Terminated` - État terminated
- `zombie_threads: Mutex<BTreeMap>` - Zombie registry
- `cleanup_zombies(max_age_ms)` - Periodic cleanup
- `reap_zombie(tid)` - Reap specific zombie
- `exit_status: i32` - Exit code storage

#### 🔴 TODO #11-15: Améliorer Zombie Handling

**TODO #11**: Automatic Resource Cleanup (3h)
```rust
// kernel/src/scheduler/thread/mod.rs
impl Drop for Thread {
    fn drop(&mut self) {
        // Cleanup:
        // - Stack deallocation
        // - File descriptors
        // - Memory mappings
        // - IPC channels
        logger::debug(&format!("[THREAD] Cleanup resources for TID {}", self.id));
    }
}
```

**TODO #12**: Parent Notification (2h)
```rust
// kernel/src/scheduler/core/scheduler.rs
pub fn notify_parent_on_exit(&self, child_tid: ThreadId, exit_code: i32) {
    if let Some(parent_tid) = self.get_parent_tid(child_tid) {
        // Send SIGCHLD to parent
        self.send_signal(parent_tid, Signal::SIGCHLD);
    }
}
```

**TODO #13**: Orphan Handling (1h)
```rust
// Si parent est mort avant child:
pub fn reparent_to_init(&self, orphan_tid: ThreadId) {
    // Reparent to PID 1 (init process)
    self.set_parent_tid(orphan_tid, 1);
}
```

**TODO #14**: Zombie Leak Prevention (1h)
```rust
// Periodic cleanup automatique
pub fn periodic_zombie_cleanup(&self) {
    // Call cleanup_zombies() every 10s
    const MAX_AGE_MS: u64 = 10_000;
    self.cleanup_zombies(MAX_AGE_MS);
}
```

**TODO #15**: Exit Status Propagation (1h)
```rust
// S'assurer que exit_status est correctement propagé
pub fn terminate_thread_with_status(&self, tid: ThreadId, status: i32) {
    self.with_thread(tid, |thread| {
        thread.set_exit_status(status);
        thread.set_state(ThreadState::Terminated);
    });
}
```

---

## 🧪 Tests de Validation

### FPU Tests (TODO #7)
```rust
// kernel/src/tests/fpu_tests.rs
fn test_fpu_lazy_switching() {
    // Create 2 threads: one uses FPU, one doesn't
    // Verify #NM only triggered for FPU thread
}

fn test_fpu_state_preservation() {
    // Thread A sets FPU register
    // Context switch to B
    // Context switch back to A
    // Verify FPU register unchanged
}

fn test_fpu_multi_thread() {
    // 10 threads all use FPU simultaneously
    // Verify no state corruption
}
```

### Blocked Threads Tests (TODO #10)
```rust
// kernel/src/tests/condvar_tests.rs
fn test_condvar_wait_notify() {
    // Thread A waits on condvar
    // Thread B signals condvar
    // Verify A wakes up
}

fn test_condvar_broadcast() {
    // 10 threads wait on condvar
    // notify_all()
    // Verify all wake up
}
```

### Zombie Tests (TODO #15)
```rust
// kernel/src/tests/zombie_tests.rs
fn test_zombie_cleanup() {
    // Create 100 threads
    // All terminate
    // Verify zombies cleaned up
}

fn test_zombie_reap() {
    // Parent creates child
    // Child exits
    // Parent calls wait()
    // Verify zombie reaped
}
```

---

## 📊 Métriques de Succès

### Performance
- [ ] FPU lazy switching: Save 50-100 cycles/context switch
- [ ] Zombie cleanup: < 100 zombies en production
- [ ] Blocked threads: Wake latency < 10μs

### Stabilité
- [ ] 0 memory leaks (regression tests)
- [ ] 0 FPU state corruption
- [ ] 0 zombie accumulation

### Couverture Tests
- [ ] FPU: 3 tests (lazy, preservation, multi-thread)
- [ ] CondVar: 2 tests (notify_one, notify_all)
- [ ] Zombies: 2 tests (cleanup, reap)
- [ ] Total: 7 nouveaux tests

---

## 🗓️ Planning

### Jour 1-2: FPU Integration (10h)
- [x] Infrastructure review
- [ ] TODO #1: Thread struct (1h)
- [ ] TODO #2: set_task_switched (30min)
- [ ] TODO #3: #NM handler (2h)
- [ ] TODO #4: IDT registration (30min)
- [ ] TODO #5: Lazy save (2h)
- [ ] TODO #6: Init new threads (1h)
- [ ] TODO #7: Tests (3h)

### Jour 3: Blocked Threads (8h)
- [ ] TODO #8: CondVar (3h)
- [ ] TODO #9: Timeout (3h)
- [ ] TODO #10: Broadcast (2h)

### Jour 4: Termination Cleanup (8h)
- [ ] TODO #11: Resource cleanup (3h)
- [ ] TODO #12: Parent notify (2h)
- [ ] TODO #13: Orphans (1h)
- [ ] TODO #14: Leak prevention (1h)
- [ ] TODO #15: Status propagation (1h)

---

## ✅ Checklist Completion

### Groupe A: FPU (0/7)
- [ ] TODO #1: Thread struct FPU fields
- [ ] TODO #2: set_task_switched() call
- [ ] TODO #3: #NM handler implementation
- [ ] TODO #4: IDT registration
- [ ] TODO #5: Lazy FPU save
- [ ] TODO #6: FPU init for new threads
- [ ] TODO #7: FPU tests suite

### Groupe B: Blocked Threads (0/3)
- [ ] TODO #8: CondVar implementation
- [ ] TODO #9: Timeout support
- [ ] TODO #10: Broadcast wake

### Groupe C: Termination (0/5)
- [ ] TODO #11: Resource cleanup (Drop)
- [ ] TODO #12: Parent notification
- [ ] TODO #13: Orphan handling
- [ ] TODO #14: Zombie leak prevention
- [ ] TODO #15: Exit status propagation

---

## 🚀 Prochaine Action

**Commencer TODO #1**: Ajouter FPU state dans Thread struct

```bash
# Étapes:
1. Modifier kernel/src/scheduler/thread/mod.rs
2. Ajouter champs fpu_state, fpu_used
3. Initialiser dans Thread::new_*()
4. Compiler et tester
```

**Status**: 🟢 Ready to start
