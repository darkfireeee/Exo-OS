# Scheduler Sync — WaitQueue, Mutex, RwLock, SpinLock, CondVar, Barrier

> **Sources** : `kernel/src/scheduler/sync/`  
> **Règles** : SCHED-05, SCHED-10, SCHED-11, WAITQ-01

---

## Table des matières

1. [spinlock.rs — SpinLock et IrqSpinLock](#1-spinlockrs--spinlock-et-irqspinlock)
2. [wait_queue.rs — WaitQueue](#2-wait_queuers--waitqueue)
3. [mutex.rs — KMutex](#3-mutexrs--kmutex)
4. [rwlock.rs — KRwLock](#4-rwlockrs--krwlock)
5. [condvar.rs — CondVar](#5-condvarrs--condvar)
6. [barrier.rs — KBarrier](#6-barrierrs--kbarrier)

---

## 1. spinlock.rs — SpinLock et IrqSpinLock

### SpinLock<T>

```rust
pub struct SpinLock<T> {
    locked: AtomicBool,
    data:   UnsafeCell<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self

    // Spin jusqu'à acquisition (busy-wait)
    pub fn lock(&self) -> SpinLockGuard<'_, T>

    // Tentative non-bloquante
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>>
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    data: *mut T,
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Release)
    }
}
```

Implémentation : compare-and-swap sur `AtomicBool` avec `Acquire` au lock, `Release` à l'unlock. Pause CPU (`rep nop` / `pause`) dans la boucle de spin.

### IrqSpinLock<T>

```rust
pub struct IrqSpinLock<T> {
    inner: SpinLock<T>,
}

impl<T> IrqSpinLock<T> {
    pub const fn new(value: T) -> Self

    // Désactive les IRQ + acquiert le spinlock
    pub fn lock_irq(&self) -> IrqSpinLockGuard<'_, T>

    pub fn try_lock_irq(&self) -> Option<IrqSpinLockGuard<'_, T>>
}

pub struct IrqSpinLockGuard<'a, T> {
    guard:        SpinLockGuard<'a, T>,
    saved_rflags: u64,  // RFLAGS avant CLI
}

impl<T> Drop for IrqSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // 1. Unlock inner SpinLock (drop guard)
        // 2. Restaure RFLAGS (réactive IRQ si elles l'étaient)
    }
}
```

### Primitives IRQ bas niveau

```rust
// Sauvegarde RFLAGS + CLI, retourne ancien RFLAGS
pub fn save_and_disable_irq() -> u64

// Restaure RFLAGS depuis valeur sauvegardée
pub fn restore_irq(rflags: u64)
```

---

## 2. wait_queue.rs — WaitQueue

### WaitNode (SCHED-05, WAITQ-01)

```rust
#[repr(C)]
pub struct WaitNode {
    tcb:   *mut ThreadControlBlock,
    next:  *mut WaitNode,
    prev:  *mut WaitNode,
    flags: u32,          // EXCLUSIVE ou 0
    _pad:  u32,
}
// Taille = 32 octets (4×8 + 4 + 4)
```

**WaitNodes uniquement depuis EmergencyPool** (jamais depuis l'allocateur heap) :

```rust
impl WaitNode {
    pub const EXCLUSIVE: u32 = 1 << 0;  // Réveil exclusif (1 seul thread)

    // Alloue depuis emergency_pool (FFI memory/)
    pub unsafe fn alloc(
        tcb: *mut ThreadControlBlock,
        flags: u32,
    ) -> Option<NonNull<WaitNode>>

    // Libre dans emergency_pool
    pub unsafe fn free(node: NonNull<WaitNode>)
}
```

### FFI EmergencyPool (WAITQ-01)

```rust
extern "C" {
    fn emergency_pool_alloc_wait_node() -> *mut WaitNode;
    fn emergency_pool_free_wait_node(node: *mut WaitNode);
}
```

### WaitQueue

```rust
pub struct WaitQueue {
    head:  *mut WaitNode,   // Liste doublement chaînée
    count: usize,
    lock:  SpinLock<()>,
}

impl WaitQueue {
    pub const fn new() -> Self

    // Insère le nœud en fin de queue
    pub unsafe fn insert(&mut self, node: NonNull<WaitNode>)

    // Retire un nœud spécifique
    pub unsafe fn remove(&mut self, node: NonNull<WaitNode>)

    // Réveille le premier thread (ou premier EXCLUSIVE)
    // Retourne true si un thread a été réveillé
    pub unsafe fn wake_one(&mut self) -> bool

    // Réveille tous les threads non-EXCLUSIVE
    pub unsafe fn wake_all(&mut self) -> usize

    pub fn is_empty(&self) -> bool
    pub fn count(&self) -> usize
}
```

### Séquence wait typique

```rust
// Pattern pour attendre sur une WaitQueue :
let node = WaitNode::alloc(current_tcb, 0).expect("OOM emergency pool");
wq.insert(node);
current_tcb.set_state(Blocked);
schedule_yield(current_tcb, rq);
// ... réveil par wake_one() ...
WaitNode::free(node);
```

### Initialisation globale

```rust
pub unsafe fn init()  // Pré-alloue le pool de WaitNodes dans emergency_pool
```

### Compteurs

```rust
pub static WAITQ_WAKEUPS:  AtomicU64
pub static WAITQ_TIMEOUTS: AtomicU64
```

---

## 3. mutex.rs — KMutex

### Structure

```rust
pub struct KMutex<T> {
    owner: AtomicU32,    // ThreadId du propriétaire (0 = libre)
    waitq: WaitQueue,
    data:  UnsafeCell<T>,
}
```

### API

```rust
impl<T> KMutex<T> {
    pub const fn new(value: T) -> Self

    // Non-bloquant : succède si le mutex est libre
    pub fn try_lock(&self, tid: u32) -> Option<KMutexGuard<'_, T>>

    // Bloquant : met le thread en Blocked si le mutex est pris
    // Nécessite le TCB courant et la run queue
    pub unsafe fn lock_blocking(
        &self,
        tid: u32,
        current: &mut ThreadControlBlock,
        rq: &mut PerCpuRunQueue,
    ) -> KMutexGuard<'_, T>

    pub fn is_locked(&self) -> bool
    pub fn owner(&self) -> u32         // ThreadId du propriétaire actuel
}

pub struct KMutexGuard<'a, T> {
    mutex: &'a KMutex<T>,
    data:  *mut T,
}

impl<T> Drop for KMutexGuard<'_, T> {
    fn drop(&mut self) {
        // 1. owner = 0 (libère)
        // 2. waitq.wake_one() si des threads attendent
    }
}
```

### Séquence lock_blocking

```
1. try_lock() → succès → retourne guard
2. Sinon :
   a. Alloue WaitNode depuis EmergencyPool
   b. Insère dans waitq
   c. set_state(Blocked)
   d. schedule_yield() ← abandon CPU
   e. Répète depuis 1 (réessai après réveil)
   f. WaitNode::free()
```

**Pas de priority inheritance** dans l'implémentation actuelle (future évolution).

### Compteurs

```rust
pub static KMUTEX_CONTENTIONS: AtomicU64  // Acquisitions en attente
pub static KMUTEX_ACQUIRES:    AtomicU64  // Acquisitions totales
```

---

## 4. rwlock.rs — KRwLock

```rust
pub struct KRwLock<T> {
    state: AtomicI32,   // >0 = lecteurs actifs, -1 = écrivain, 0 = libre
    data:  UnsafeCell<T>,
}

impl<T> KRwLock<T> {
    pub const fn new(value: T) -> Self

    // Acquiert en lecture (spin si écrivain actif)
    pub fn read(&self) -> KReadGuard<'_, T>

    // Acquiert en écriture (exclusif — spin si lecteurs ou écrivain actif)
    pub fn write(&self) -> KWriteGuard<'_, T>
}

pub struct KReadGuard<'a, T>  { /* &T */ }
pub struct KWriteGuard<'a, T> { /* &mut T */ }

impl Drop for KReadGuard<'_, T>  { /* state -= 1 */ }
impl Drop for KWriteGuard<'_, T> { /* state = 0  */ }
```

Implémentation : spin-based (pas de park). Convient aux sections critiques courtes. Pour les longues attentes, préférer `KMutex`.

---

## 5. condvar.rs — CondVar

### Structure

```rust
pub struct CondVar {
    waiters: WaitQueue,
    seq:     AtomicU64,  // Numéro de séquence (détection spurious)
}
```

### API

```rust
impl CondVar {
    pub const fn new() -> Self

    // Réveille un waiter
    pub unsafe fn notify_one(&mut self)

    // Réveille tous les waiters
    pub unsafe fn notify_all(&mut self)

    // Attend sur la CondVar en relâchant le mutex
    // Retourne KMutexGuard (mutex ré-acquis)
    pub unsafe fn wait_on<'a, T>(
        &mut self,
        guard: KMutexGuard<'a, T>,
        current: &mut ThreadControlBlock,
        rq: &mut PerCpuRunQueue,
    ) -> KMutexGuard<'a, T>

    pub fn seq(&self) -> u64              // Numéro de séquence courant
    pub fn waiters_count(&self) -> usize
}
```

### Séquence wait_on

```
1. Enregistre seq_before = self.seq.load()
2. Insère WaitNode dans self.waiters
3. Drop guard (libère le mutex)
4. set_state(Blocked) + schedule_yield()
5. Après réveil : ré-acquiert le mutex (lock_blocking)
6. Retourne nouveau guard
```

### Compteurs

```rust
pub static CONDVAR_WAITS:    AtomicU64  // Entrées en attente
pub static CONDVAR_WAKEUPS:  AtomicU64  // Sorties par notify
pub static CONDVAR_SPURIOUS: AtomicU64  // Réveils spurieux détectés
```

---

## 6. barrier.rs — KBarrier

```rust
pub struct KBarrier {
    count:  AtomicU32,  // Nombre de threads encore à attendre
    total:  u32,        // Valeur initiale
    gen:    AtomicU32,  // Génération (pour réutilisation)
}

impl KBarrier {
    pub const fn new(n: u32) -> Self

    // Attend que n threads aient appelé wait()
    // Retourne true pour le dernier thread (celui qui "ouvre" la barrière)
    pub fn wait(&self) -> bool {
        let remaining = self.count.fetch_sub(1, SeqCst);
        if remaining == 1 {
            // Dernier thread : remet le compteur pour réutilisation
            self.count.store(self.total, SeqCst);
            self.gen.fetch_add(1, SeqCst);
            return true;
        }
        // Spin jusqu'à changement de génération
        let gen = self.gen.load(SeqCst);
        while self.gen.load(Relaxed) == gen {
            core::hint::spin_loop();
        }
        false
    }

    // Remet le compteur à total (hors usage concurrent)
    pub fn reset(&self)
}
```

Utilisation typique : synchronisation des CPUs secondaires lors de l'initialisation SMP.
