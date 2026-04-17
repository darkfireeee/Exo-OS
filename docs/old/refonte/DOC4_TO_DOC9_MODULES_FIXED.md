# 📋 DOC 4 — MODULE PROCESS/ + SIGNAL/ : CONCEPTION COMPLÈTE
> Exo-OS · Couche 1.5 · Entre scheduler et ipc
> Règles anti-crash · POSIX compliance · gestion cycle de vie

---

## POSITION DANS L'ARCHITECTURE

```
┌─────────────────────────────────────────────────────────┐
│  process/  ← COUCHE 1.5                                  │
│                                                         │
│  DÉPEND DE : memory/ + scheduler/                       │
│  APPELLE fs/ : uniquement via trait abstrait (exec ELF) │
│  EST APPELÉ PAR : ipc/, fs/, arch/syscall               │
│  signal/ : ICI (déplacé depuis scheduler/ — voir DOC1)  │
└─────────────────────────────────────────────────────────┘
```

---

## ARBORESCENCE COMPLÈTE

```
kernel/src/process/
├── mod.rs
│
├── core/
│   ├── mod.rs
│   ├── pid.rs              # PID allocator (IDR radix tree, lock-free)
│   ├── pcb.rs              # Process Control Block (cache-aligned)
│   ├── tcb.rs              # Thread Control Block + ThreadAiState
│   └── registry.rs         # Registre global PID→PCB (RCU protégé)
│
├── lifecycle/
│   ├── mod.rs
│   ├── create.rs           # Création process/thread
│   ├── fork.rs             # fork() CoW <1µs
│   ├── exec.rs             # execve() via trait ElfLoader abstrait
│   ├── exit.rs             # exit() RAII cleanup complet
│   ├── wait.rs             # waitpid/wait4
│   └── reap.rs             # Reaper zombie (kthread dédié)
│
├── thread/
│   ├── mod.rs
│   ├── creation.rs         # Thread <500ns
│   ├── join.rs             # Thread join (futex-based)
│   ├── detach.rs
│   ├── local_storage.rs    # TLS (GS register, per-thread)
│   └── pthread_compat.rs   # API pthread POSIX
│
├── state/
│   ├── mod.rs
│   ├── transitions.rs      # State machine: Running/Sleep/Stop/Zombie
│   └── wakeup.rs           # wakeup_thread() — impl DmaWakeupHandler trait
│                           # Enregistré auprès de memory/dma/ au boot
│
├── signal/                 # ← DÉPLACÉ DE scheduler/ (voir DOC1)
│   ├── mod.rs
│   ├── delivery.rs         # Livraison signal au retour kernel uniquement
│   ├── handler.rs          # Exécution handler utilisateur (sigaltstack)
│   ├── mask.rs             # sigprocmask — masque par thread
│   ├── queue.rs            # File RT signals (POSIX.1b)
│   └── default.rs          # Actions: TERM, CORE, IGN, STOP, CONT
│
├── group/
│   ├── mod.rs
│   ├── session.rs
│   ├── pgrp.rs
│   └── job_control.rs
│
├── namespace/
│   ├── mod.rs
│   ├── pid_ns.rs
│   ├── mount_ns.rs
│   ├── net_ns.rs
│   ├── uts_ns.rs
│   └── user_ns.rs
│
└── resource/
    ├── mod.rs
    ├── rlimit.rs
    ├── usage.rs
    └── cgroup.rs           # cgroups v2
```

---

## RÈGLES CRITIQUES

### 📌 process/core/tcb.rs

```rust
// kernel/src/process/core/tcb.rs
//
// ThreadControlBlock — structure centrale, cache-aligned
// DOIT tenir dans 2 cache lines (128 bytes) pour le hot path

#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // === Cache line 1 (64 bytes) — hot path scheduler ===
    pub thread_id:       ThreadId,          // 8 bytes
    pub process_id:      Pid,               // 4 bytes
    pub state:           AtomicU8,          // 1 byte (Running/Sleeping/Zombie)
    pub policy:          SchedPolicy,       // 1 byte
    pub cpu_affinity:    CpuMask,           // 8 bytes
    pub vruntime:        u64,               // 8 bytes (CFS)
    pub saved_rsp:       u64,               // 8 bytes (context switch)
    pub signal_pending:  AtomicBool,        // 1 byte (LU par scheduler, ÉCRIT par process/signal/)
    pub signal_mask:     AtomicU64,         // 8 bytes (bitmask signaux bloqués)
    pub fpu_used:        bool,              // 1 byte
    pub fpu_saved:       bool,              // 1 byte
    // ✅ CORRIGÉ: _pad1 recalculé pour atteindre exactement 64 bytes
    // thread_id(8)+process_id(4)+state(1)+policy(1)+cpu_affinity(8)+
    // vruntime(8)+saved_rsp(8)+signal_pending(1)+signal_mask(8)+
    // fpu_used(1)+fpu_saved(1) = 49 bytes → pad = 15 bytes ✓
    _pad1:               [u8; 15],

    // === Cache line 2 (64 bytes) — données secondaires ===
    pub address_space:   Option<AddressSpaceRef>,  // 8 bytes
    pub fpu_state:       *mut FpuState,     // 8 bytes (alloué séparément, 512B aligné)
    pub ai_state:        ThreadAiState,     // 8 bytes (inline, voir DOC1)
    pub rt_latency_us:   u32,              // 4 bytes (C-state constraint)
    pub preempt_count:   u32,              // 4 bytes (PreemptGuard counter)
    pub kernel_stack:    VirtAddr,         // 8 bytes
    pub tls_base:        VirtAddr,         // 8 bytes (GS base pour TLS)
    pub dma_completion_result: AtomicU8,   // ✅ AJOUT: résultat DMA (utilisé par wakeup.rs)
    _pad2:               [u8; 15],         // ✅ CORRIGÉ: pad recalculé (8+8+8+4+4+8+8+1=49 → pad=15)
}

const _: () = assert!(
    size_of::<ThreadControlBlock>() <= 128,
    "TCB dépasse 128 bytes — risque de cache miss dans pick_next_task"
);
```

> ⚠️ **ERREURS CORRIGÉES dans TCB :**
> - `_pad2: [u8; 16]` dans l'original → incorrect si `dma_completion_result` est ajouté.
>   Le champ `dma_completion_result: AtomicU8` (1 byte) est nécessaire pour `wakeup.rs`
>   (référencé dans le code du module mais absent de la struct). Padding recalculé à 15.
> - La struct sans `dma_completion_result` ne peut pas compiler avec `wakeup.rs` tel qu'écrit.

---

### 📌 process/lifecycle/exec.rs

**RÈGLE EXEC-01 : execve() via trait abstrait — pas d'import fs/ direct**

```rust
// kernel/src/process/lifecycle/exec.rs
//
// PROBLÈME : exec() doit charger un ELF depuis fs/
// CONTRAINTE : process/ (couche 1.5) ne peut pas importer fs/ (couche 3)
// SOLUTION : trait ElfLoader enregistré par fs/ au boot

pub trait ElfLoader: Send + Sync {
    fn load_elf(
        &self,
        path: &str,
        addr_space: &mut AddressSpace,
    ) -> Result<VirtAddr, ExecError>;
}

static ELF_LOADER: spin::Once<&'static dyn ElfLoader> = spin::Once::new();

pub fn register_elf_loader(loader: &'static dyn ElfLoader) {
    ELF_LOADER.call_once(|| loader);
}

pub fn do_execve(
    tcb: &mut ThreadControlBlock,
    path: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<(), ExecError> {
    let addr_space = tcb.address_space.as_mut().unwrap();
    addr_space.clear()?;

    let loader = ELF_LOADER.get()
        .ok_or(ExecError::ElfLoaderNotRegistered)?;
    let entry_point = loader.load_elf(path, addr_space)?;

    setup_initial_stack(addr_space, argv, envp)?;

    tcb.signal_mask.store(0, Ordering::Release);
    tcb.fpu_used = false;
    tcb.ai_state = ThreadAiState::default();

    setup_return_to_user(tcb, entry_point);
    Ok(())
}
```

---

### 📌 process/signal/delivery.rs

**RÈGLE SIGNAL-01 : Livraison au retour kernel uniquement**

```rust
// kernel/src/process/signal/delivery.rs
//
// La livraison des signaux se fait UNIQUEMENT au retour vers userspace.
// JAMAIS depuis le hot path du scheduler.

pub fn handle_pending_signals(tcb: &mut ThreadControlBlock) {
    if !tcb.signal_pending.load(Ordering::Acquire) {
        return;
    }

    let mask = tcb.signal_mask.load(Ordering::Acquire);

    while let Some(sig) = dequeue_signal(tcb, mask) {
        deliver_signal(tcb, sig);
    }

    if is_signal_queue_empty(tcb) {
        tcb.signal_pending.store(false, Ordering::Release);
    }
}

pub fn send_signal(target: &ThreadControlBlock, signal: Signal) {
    enqueue_signal(target, signal);
    target.signal_pending.store(true, Ordering::Release);

    if target.state.load(Ordering::Acquire) == TaskState::Sleeping as u8 {
        // ✅ CORRECT : process/signal/ appelle scheduler/sync/wait_queue
        // Direction autorisée : process/ → scheduler/ (couche 1.5 → couche 1)
        scheduler::sync::wait_queue::wake_thread(target.thread_id);
    }
}

fn deliver_signal(tcb: &mut ThreadControlBlock, sig: Signal) {
    let action = tcb.signal_actions[sig as usize];
    match action {
        SignalAction::Default       => default::handle_default(tcb, sig),
        SignalAction::Ignore        => {},
        SignalAction::Handler(ptr)  => handler::setup_signal_frame(tcb, sig, ptr),
    }
}
```

---

### 📌 process/state/wakeup.rs

**RÈGLE WAKEUP-01 : Implémenter DmaWakeupHandler — enregistré au boot**

```rust
// kernel/src/process/state/wakeup.rs

struct ProcessWakeupHandler;

impl memory::dma::core::wakeup_iface::DmaWakeupHandler for ProcessWakeupHandler {
    fn wakeup_thread(&self, thread_id: ThreadId, result: Result<(), DmaError>) {
        let tcb = match process::core::registry::get_tcb(thread_id) {
            Some(t) => t,
            None => return,  // Thread terminé entre-temps — safe
        };

        // Stocker le résultat dans le TCB (champ dma_completion_result)
        tcb.dma_completion_result.store(result.is_ok() as u8, Ordering::Release);

        if tcb.state.load(Ordering::Acquire) == TaskState::Sleeping as u8 {
            scheduler::sync::wait_queue::wake_thread(thread_id);
        }
    }
}

static PROCESS_WAKEUP_HANDLER: ProcessWakeupHandler = ProcessWakeupHandler;

/// Appelé au boot (step 20 dans la séquence globale)
pub fn register_with_dma() {
    memory::dma::core::wakeup_iface::register_wakeup_handler(
        &PROCESS_WAKEUP_HANDLER
    );
}
```

---

## TABLEAU DES RÈGLES PROCESS/

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — process/ (couche 1.5)                        │
├────────────────────────────────────────────────────────────────┤
│ PROC-01 │ exec() via trait ElfLoader abstrait (pas import fs/) │
│ PROC-02 │ DmaWakeupHandler impl ici, enregistré au boot        │
│ PROC-03 │ signal/ géré ici entièrement (pas dans scheduler/)   │
│ PROC-04 │ signal_pending = AtomicBool — ÉCRIT par              │
│           │ process/signal/, LU par scheduler (jamais inverse)  │
│ PROC-05 │ TCB ≤ 128 bytes (2 cache lines)                      │
│ PROC-06 │ Livraison signal : au retour userspace UNIQUEMENT     │
│ PROC-07 │ zombie reaper = kthread dédié (jamais inline exit)   │
│ PROC-08 │ fork() : flush TLB parent AVANT retour               │
│ PROC-09 │ namespace/ : isolation complète PID/net/mount        │
│ PROC-10 │ dma_completion_result dans TCB — champ AtomicU8      │
│           │ (résultat stocké par ProcessWakeupHandler)          │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  use crate::fs dans process/ (sauf trait abstrait)           │
│ ✗  use crate::ipc dans process/ (sauf trait abstrait)          │
│ ✗  Livrer un signal depuis le hot path scheduler               │
│ ✗  Allouer WaitNode depuis heap dans signal/delivery.rs        │
│ ✗  TCB sans champ dma_completion_result                        │
└────────────────────────────────────────────────────────────────┘
```

---
---
---

# 📋 DOC 5 — MODULE IPC/ : CONCEPTION COMPLÈTE
> Exo-OS · Couche 2a · Dépend de memory/ + scheduler/ + security/capability/
> Zero-copy · Lock-free · Capability-gated · <700 cycles latence

---

## POSITION DANS L'ARCHITECTURE

```
┌─────────────────────────────────────────────────────────┐
│  ipc/  ← COUCHE 2a                                       │
│                                                         │
│  DÉPEND DE : memory/ + scheduler/ + security/capability  │
│  EST APPELÉ PAR : fs/ (via shim), userspace             │
│  INTERDIT : appeler fs/ directement                      │
└─────────────────────────────────────────────────────────┘
```

**OBJECTIFS :**

| Métrique | Cible |
|---|---|
| Latence IPC small msg (<40B) | 500–700 cycles |
| Throughput IPC zero-copy | >100M msgs/s |
| Zero-copy large msg | >50 GB/s |

---

## ARBORESCENCE COMPLÈTE

```
kernel/src/ipc/
├── mod.rs
│
├── core/
│   ├── mod.rs
│   ├── types.rs                # MessageId, ChannelId, EndpointId, Cookie
│   ├── fastcall_asm.s          # Fast IPC sans syscall (ASM pur — fichier .s)
│   ├── transfer.rs             # Message transfer engine
│   ├── sequence.rs             # Numéros de séquence
│   └── constants.rs            # MAX_MSG_SIZE=4080B, RING_SIZE=4096
│
├── channel/
│   ├── mod.rs
│   ├── sync.rs                 # Canal synchrone (rendezvous)
│   ├── async.rs                # Canal async (futures/waker)
│   ├── mpmc.rs                 # MPMC
│   ├── broadcast.rs            # One-to-many
│   ├── typed.rs                # Type-safe
│   └── streaming.rs            # Streaming DMA
│
├── ring/
│   ├── mod.rs
│   ├── spsc.rs                 # SPSC ultra-fast (CachePadded OBLIGATOIRE)
│   ├── mpmc.rs                 # MPMC lock-free
│   ├── fusion.rs               # Fusion Ring — adaptive batching (anti-thundering herd)
│   ├── slot.rs                 # Slot management
│   ├── batch.rs                # Batch transfers
│   └── zerocopy.rs             # Zero-copy (partage page physique)
│
├── shared_memory/
│   ├── mod.rs
│   ├── pool.rs                 # Pool pré-alloué
│   ├── mapping.rs              # Mapping dans espaces d'adressage
│   ├── page.rs                 # NO_COW + SHM_PINNED obligatoires
│   ├── descriptor.rs           # SHM descriptor
│   ├── allocator.rs            # Lock-free alloc SHM
│   └── numa_aware.rs           # NUMA locality pour SHM
│
├── capability_bridge/          # Shim — security/capability/ = source réelle
│   ├── mod.rs                  # ⚠️ Zéro logique ici — délégation pure
│   └── bridge.rs               # verify_ipc_access() → security::capability::verify()
│
├── endpoint/
│   ├── mod.rs
│   ├── descriptor.rs           # EndpointDesc (ID, owners, queue)
│   ├── registry.rs             # Registre nom→endpoint (radix tree)
│   ├── connection.rs           # Établissement connexion + handshake
│   └── lifecycle.rs            # Création/destruction + cleanup
│
├── sync/
│   ├── mod.rs
│   ├── futex.rs                # Délégation pure → memory/utils/futex_table (zéro logique locale)
│   ├── wait_queue.rs           # Délègue à scheduler/sync/wait_queue
│   ├── event.rs                # Event notification
│   ├── barrier.rs              # Barrière de synchronisation
│   └── rendezvous.rs           # Point de rendez-vous
│
├── message/
│   ├── mod.rs
│   ├── builder.rs              # Constructeur fluent de messages
│   ├── serializer.rs           # Zero-copy (capnproto-like)
│   ├── router.rs               # Routage multi-hop
│   └── priority.rs             # Priorité messages (RT/normal)
│
├── rpc/
│   ├── mod.rs
│   ├── server.rs               # RPC server (dispatcher)
│   ├── client.rs               # RPC client (stub + await)
│   ├── protocol.rs             # Protocole RPC binaire
│   └── timeout.rs              # Timeout + retry
│
└── stats/
    ├── mod.rs                  # ✅ AJOUT: mod.rs manquant dans l'original
    └── counters.rs             # Throughput msgs/s, latences, drops
```

---

## RÈGLES CRITIQUES IPC

### 📌 ipc/ring/spsc.rs — CachePadded OBLIGATOIRE

```rust
// kernel/src/ipc/ring/spsc.rs
//
// RÈGLE ANTI-FALSE-SHARING :
// head et tail sur des cache lines SÉPARÉES
// Sans CachePadded → false sharing → dégradation 10-100×

#[repr(C, align(64))]
struct CachePadded<T> {
    value: T,
    _pad: [u8; 64 - size_of::<T>() % 64],
}

pub struct SpscRing<T, const N: usize> {
    head: CachePadded<AtomicU64>,   // Cache line producteur
    tail: CachePadded<AtomicU64>,   // Cache line consommateur (SÉPARÉE)
    buffer: [MaybeUninit<T>; N],
}

impl<T, const N: usize> SpscRing<T, N> {
    const MASK: u64 = N as u64 - 1;

    #[inline(always)]
    pub fn push(&self, item: T) -> Result<(), T> {
        let head = self.head.value.load(Ordering::Relaxed);
        let tail = self.tail.value.load(Ordering::Acquire);

        if head.wrapping_sub(tail) >= N as u64 {
            return Err(item);  // Ring plein
        }

        unsafe {
            (self.buffer[(head & Self::MASK) as usize].as_ptr() as *mut T).write(item);
        }

        self.head.value.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    #[inline(always)]
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.value.load(Ordering::Relaxed);
        let head = self.head.value.load(Ordering::Acquire);

        if head == tail {
            return None;  // Ring vide
        }

        let item = unsafe {
            (self.buffer[(tail & Self::MASK) as usize].as_ptr()).read()
        };

        self.tail.value.store(tail.wrapping_add(1), Ordering::Release);
        Some(item)
    }
}

// Vérification statique : N doit être puissance de 2
const fn is_power_of_two(n: usize) -> bool { n != 0 && (n & (n - 1)) == 0 }
```

---

### 📌 ipc/ring/fusion.rs — Anti-thundering herd

```rust
// kernel/src/ipc/ring/fusion.rs
//
// Fusion Ring — adaptive batching
// N messages → 1 wakeup (pas N wakeups → N context switches)

pub struct FusionRing<T, const N: usize> {
    ring: SpscRing<T, N>,
    batch_threshold: AtomicU32,
    pending_count:   AtomicU32,
    consumer_waker:  AtomicPtr<WakeupToken>,
}

impl<T, const N: usize> FusionRing<T, N> {
    pub fn send(&self, item: T) -> Result<(), T> {
        self.ring.push(item)?;

        let count = self.pending_count.fetch_add(1, Ordering::Relaxed) + 1;
        let threshold = self.batch_threshold.load(Ordering::Relaxed);

        if count >= threshold || self.ring.len() > N * 3 / 4 {
            self.pending_count.store(0, Ordering::Relaxed);
            self.wake_consumer();
        }
        Ok(())
    }

    /// Ajuster le seuil selon la latence observée
    pub fn adjust_batch_threshold(&self, observed_latency_us: u32) {
        let current = self.batch_threshold.load(Ordering::Relaxed);
        let new_threshold = if observed_latency_us > 100 {
            (current / 2).max(1)   // Trop de latence → réduire batch
        } else if observed_latency_us < 10 {
            (current * 2).min(64)  // Faible latence → augmenter batch
        } else {
            return;
        };
        self.batch_threshold.store(new_threshold, Ordering::Relaxed);
    }
}
```

---

### 📌 ipc/shared_memory/page.rs — NO_COW obligatoire

```rust
// RÈGLE : NO_COW + SHM_PINNED sur TOUTES les pages SHM
// Raison : page SHM marquée CoW → fork() la copie → fils perd le canal silencieusement

pub fn alloc_shm_page() -> Result<ShmPage, ShmError> {
    let frame = memory::physical::allocator::buddy::alloc_pages(
        0,
        AllocFlags::KERNEL | AllocFlags::ZERO,
    )?;

    frame.flags.insert(FrameFlags::NO_COW);      // Anti-CoW obligatoire
    frame.flags.insert(FrameFlags::SHM_PINNED);  // Anti-eviction

    Ok(ShmPage {
        frame,
        phys_addr: frame.phys_addr(),
    })
}
```

---

### 📌 ipc/sync/futex.rs — Délégation UNIQUEMENT

```rust
// CE MODULE NE CONTIENT AUCUNE LOGIQUE DE FUTEX
// Délégation pure à memory/utils/futex_table

pub fn ipc_futex_wait(phys_addr: PhysAddr, expected: u32, timeout: Option<Duration>) -> FutexResult {
    memory::utils::futex_table::FUTEX_TABLE.wait(phys_addr, expected, timeout)
}

pub fn ipc_futex_wake(phys_addr: PhysAddr, count: u32) -> u32 {
    memory::utils::futex_table::FUTEX_TABLE.wake(phys_addr, count)
}
```

---

## TABLEAU DES RÈGLES IPC/

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — ipc/ (couche 2a)                             │
├────────────────────────────────────────────────────────────────┤
│ IPC-01 │ SPSC ring : head et tail sur cache lines SÉPARÉES     │
│          │ (CachePadded<AtomicU64>)                            │
│ IPC-02 │ futex.rs = délégation à memory/utils/futex_table      │
│          │ AUCUNE logique futex locale                          │
│ IPC-03 │ Pages SHM = FrameFlags::NO_COW + SHM_PINNED          │
│          │ (les deux obligatoires)                              │
│ IPC-04 │ capability_bridge/ délègue à security/capability/     │
│          │ AUCUNE logique de droits locale                      │
│ IPC-05 │ ipc/ N'APPELLE PAS fs/ directement                   │
│          │ (passer par fs/ipc_fs/shim.rs uniquement)           │
│ IPC-06 │ Fusion Ring : anti-thundering herd (batch adaptatif)  │
│ IPC-07 │ Fast IPC = fichier .s ASM (pas .rs)                  │
│ IPC-08 │ Spectre v1 : array_index_nospec() sur accès buffers   │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Table futex locale dans ipc/                                │
│ ✗  Logique capability dans ipc/ (capability_bridge/ = shim)    │
│ ✗  Pages SHM sans NO_COW                                       │
│ ✗  Dépendance directe sur fs/                                  │
│ ✗  SPSC sans CachePadded (false sharing)                       │
└────────────────────────────────────────────────────────────────┘
```

---
---
---

# 📋 DOC 6 — MODULE FS/ : CONCEPTION COMPLÈTE
> Exo-OS · Couche 3 · SEUL module en Ring 0 hors TCB
> Performance I/O · Journaling · io_uring natif

---

## POSITION DANS L'ARCHITECTURE

```
┌─────────────────────────────────────────────────────────┐
│  fs/  ← COUCHE 3 — SEUL en Ring 0 (décision archit.)    │
│                                                         │
│  DÉPEND DE : memory/ + scheduler/ + security/capability  │
│  IPC via : fs/ipc_fs/shim.rs (shim obligatoire)         │
│  NE PEUT PAS : être appelé par scheduler/ ou memory/    │
└─────────────────────────────────────────────────────────┘
```

---

## RÈGLES CRITIQUES FS/

### 📌 fs/core/inode.rs — Pattern release-before-sleep

```rust
// RÈGLE ANTI-DEADLOCK : Lock inversion inode × wait_queue
// TOUJOURS relâcher le lock inode AVANT de dormir

pub struct Inode {
    lock:      RwLock<InodeData>,
    wait_lock: SpinLock<WaitList>,  // JAMAIS tenu en même temps que 'lock'
    ref_count: AtomicU32,
}

impl Inode {
    pub fn read(&self, buf: &mut [u8], offset: u64) -> Result<usize, FsError> {
        let data = self.lock.read();

        if data.is_cached(offset) {
            return Ok(data.read_cached(buf, offset));
        }

        drop(data);  // ← CRITIQUE: relâcher AVANT sleep (release-before-sleep)

        let io_cookie = self.submit_io_read(offset, buf.len())?;
        self.wait_for_io(io_cookie)?;

        let data = self.lock.read();
        Ok(data.read_cached(buf, offset))
    }
}
```

---

### 📌 fs/io/uring.rs — EINTR propre

```rust
// RÈGLE : sleep_interruptible() + IORING_OP_ASYNC_CANCEL pour EINTR propre

pub fn submit_and_wait(ring: &mut IoUring, sqe: SubmissionQueueEntry) -> Result<i32, IoError> {
    ring.submit(sqe)?;

    loop {
        match ring.wait_completion_interruptible() {
            Ok(cqe) => return Ok(cqe.result),
            Err(IoError::Interrupted) => {
                ring.submit_cancel(sqe.user_data)?;
                return Err(IoError::Interrupted);
            }
            Err(e) => return Err(e),
        }
    }
}
```

---

### 📌 fs/ipc_fs/shim.rs — SEUL chemin IPC→FS

```rust
// RÈGLE ABSOLUE : ipc/ → fs/ipc_fs/shim.rs → fs/core/vfs.rs
// ipc/ n'importe PAS fs/ directement

pub fn create_pipe() -> Result<(PipeFd, PipeFd), FsError> {
    let inode = fs::core::vfs::create_pipe_inode()?;
    Ok((PipeFd::ReadEnd(inode.clone()), PipeFd::WriteEnd(inode)))
}
```

---

## TABLEAU DES RÈGLES FS/

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — fs/ (couche 3)                               │
├────────────────────────────────────────────────────────────────┤
│ FS-01  │ Relâcher lock inode AVANT sleep (release-before-sleep) │
│ FS-02  │ io_uring : EINTR propre avec IORING_OP_ASYNC_CANCEL   │
│ FS-03  │ IPC via shim UNIQUEMENT (fs/ipc_fs/shim.rs)           │
│ FS-04  │ Capabilities : via security/capability/ (pas ipc/)     │
│ FS-05  │ Thundering herd : completion callbacks sélectifs       │
│ FS-06  │ ElfLoader trait enregistré par fs/ pour process/exec   │
│ FS-07  │ Slab shrinker enregistré auprès de memory/shrinker.rs  │
│ FS-08  │ Blake3 checksums sur toutes les écritures ext4+        │
│ FS-09  │ WAL (Write-Ahead Log) avant toute modification méta    │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Être appelé par scheduler/ ou memory/                       │
│ ✗  Tenir un lock inode pendant un sleep                        │
│ ✗  ipc/ importe fs/ directement (passer par shim)              │
│ ✗  Écrire sans WAL d'abord                                     │
└────────────────────────────────────────────────────────────────┘
```

---
---
---

# 📋 DOC 7 — MODULE SECURITY/CAPABILITY/ : TCB + PREUVES
> Exo-OS · Périmètre TCB prouvé Coq/TLA+ · ~500 lignes
> Zero Trust · Révocation O(1) · XChaCha20

---

## PÉRIMÈTRE DE PREUVE FORMELLE

```
DANS LE PÉRIMÈTRE COQ/TLA+ (~500 lignes) :
  security/capability/model.rs
  security/capability/token.rs
  security/capability/rights.rs
  security/capability/revocation.rs
  security/capability/delegation.rs

HORS PÉRIMÈTRE (mais dans security/) :
  security/capability/table.rs      (implémentation radix tree)
  security/capability/namespace.rs
  security/crypto/                  (crypto séparée — autre preuve)
  security/zero_trust/
  security/exploit_mitigations/
```

---

## PROPRIÉTÉS PROUVÉES

```
PROP-1 (Sûreté capability) :
  ∀ token t, object o :
  verify(t, o) = Ok → t.object_id = o.id ∧ t.generation = table[o.id].generation

PROP-2 (Révocation instantanée) :
  ∀ token t, object o :
  revoke(o) ; verify(t, o) = Err(Revoked)

PROP-3 (Confinement délégation) :
  ∀ token t1, t2 :
  delegate(t1) = t2 → t2.rights ⊆ t1.rights
  (impossible de déléguer plus de droits qu'on en possède)

PROP-4 (Correction XChaCha20) :
  ∀ plaintext p, key k, nonce n :
  decrypt(k, n, encrypt(k, n, p)) = p ∧ integrity_check()
```

---

## RÈGLES SECURITY/

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — security/ (TCB)                              │
├────────────────────────────────────────────────────────────────┤
│ SEC-01 │ capability/ = source unique de vérité (PROP-1 à 4)    │
│ SEC-02 │ ipc/capability_bridge/ = shim délègue TOUT            │
│ SEC-03 │ fs/ et process/ appellent security/capability/ direct  │
│          │ ipc/ passe par ipc/capability_bridge/ uniquement     │
│ SEC-04 │ Révocation = O(1) génération++ (jamais parcours)       │
│ SEC-05 │ XChaCha20 sur TOUS les canaux inter-domaines           │
│ SEC-06 │ KASLR actif (randomisation adresse base kernel)        │
│ SEC-07 │ Retpoline sur TOUS les appels indirects hot path       │
│ SEC-08 │ SSBD per-thread, switché avec le contexte              │
│ SEC-09 │ Audit log : ring buffer, non-bloquant, tamper-proof    │
│ SEC-10 │ Périmètre preuve : ≤ 500 lignes dans capability/model  │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Modifier security/capability/model.rs sans MAJ preuves Coq  │
│ ✗  Dupliquer verify() dans un autre module                     │
│ ✗  Délégation sans vérification ⊆ de droits                   │
│ ✗  Canal inter-domaines sans XChaCha20                         │
└────────────────────────────────────────────────────────────────┘
```

---
---
---

# 📋 DOC 8 — MODULE DMA/ (sous memory/) : CONCEPTION COMPLÈTE

---

## RÈGLES CRITIQUES DMA

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — memory/dma/                                  │
├────────────────────────────────────────────────────────────────┤
│ DMA-01 │ DMA sous memory/ — dépend de physical/frame/zone     │
│ DMA-02 │ Wakeup thread = trait DmaWakeupHandler               │
│          │ (jamais import direct de process/)                   │
│ DMA-03 │ FrameFlags::DMA_PINNED jusqu'à wait_dma_complete()   │
│ DMA-04 │ TLB shootdown avant free d'un frame DMA              │
│ DMA-05 │ Huge page : INTERDIT split si DMA_PINNED             │
│ DMA-06 │ IOMMU domain par device (isolation hardware)         │
│ DMA-07 │ Completion IRQ hot path ≤ 500ns                      │
│ DMA-08 │ Ring doorbell : 1 seul write MMIO par batch          │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  DMA hors de memory/ (cycle de dépendance physique)          │
│ ✗  import process/ depuis dma/ (utiliser DmaWakeupHandler)     │
│ ✗  Libérer frame DMA_PINNED avant ACK completion              │
│ ✗  Appeler le scheduler directement depuis IRQ DMA             │
└────────────────────────────────────────────────────────────────┘
```

---
---
---

# 📋 DOC 9 — SHIELD (servers/shield/) : CONCEPTION COMPLÈTE
> Exo-OS · Ring 1 (userspace privilégié) · Protection anti-malware
> Analyse comportementale · ML embarqué · Sandbox

---

## ARCHITECTURE SHIELD

```
servers/shield/
├── engine/
│   ├── core.rs             # Orchestration principale
│   ├── realtime.rs         # Analyse temps réel (hooks syscall)
│   └── scanner.rs          # Scanner fichiers/mémoire
├── behavioral/
│   ├── profiler.rs         # Profil comportemental par processus
│   ├── anomaly.rs          # Détection anomalies (ML)
│   ├── sequence.rs         # Analyse séquences syscalls (Aho-Corasick)
│   └── heuristic.rs        # Règles heuristiques
├── ml/
│   ├── model.rs            # Modèle ONNX compact (<2MB)
│   ├── inference.rs        # Inférence <100µs
│   ├── features.rs         # Feature extraction
│   └── update.rs           # Mise à jour modèles (offline uniquement)
├── hooks/
│   ├── syscall_hooks.rs    # 47 syscalls sensibles interceptés
│   ├── memory_hooks.rs     # Allocations + exec mappings
│   ├── exec_hooks.rs       # Analyse binaires au exec
│   └── net_hooks.rs        # Trafic réseau suspect
├── sandbox/
│   ├── container.rs        # Containeurisation légère
│   ├── syscall_filter.rs   # Filtre syscalls (seccomp-like)
│   ├── net_isolation.rs    # Isolation réseau
│   └── fs_restriction.rs   # Restriction FS
├── signatures/
│   ├── database.rs         # Base signatures
│   ├── matcher.rs          # Matching Aho-Corasick
│   └── yara.rs             # Règles YARA
├── network/
│   ├── firewall.rs         # Firewall stateful
│   ├── ids.rs              # IDS
│   ├── dns_guard.rs        # Anti-C2
│   └── traffic_analysis.rs
└── ipc_gate/
    ├── policy.rs           # Politiques accès IPC
    └── audit.rs            # Log communications IPC
```

---

## RÈGLES CRITIQUES SHIELD

### Isolation du daemon lui-même

```rust
// RÈGLE SHIELD-01 : Shield est lui-même sandboxé
// Un AV compromis sans sandbox = escalade de privilèges garantie

fn self_isolate() {
    syscall_filter::apply_shield_filter();

    capability::drop_all_except(&[
        Cap::SYS_PTRACE,   // Inspection des processus
        Cap::NET_ADMIN,    // Firewall
        Cap::AUDIT_WRITE,  // Logs
    ]);

    // Watchdog obligatoire : si shield plante → redémarrage automatique
    // Sans watchdog → protection silencieuse après crash (SHIELD-02)
    watchdog::register_shield(Duration::from_secs(30));
}
```

---

## TABLEAU DES RÈGLES SHIELD

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — servers/shield/                              │
├────────────────────────────────────────────────────────────────┤
│ SHL-01 │ Shield est lui-même sandboxé (self_isolate() au boot) │
│ SHL-02 │ Watchdog obligatoire (protection ne tombe pas si crash)│
│ SHL-03 │ ML inference ≤ 100µs (modèle ≤ 2MB)                  │
│ SHL-04 │ Hooks = analyse, pas blocage systématique             │
│ SHL-05 │ Pas de duplication crypto (appel syscall → kernel)    │
│ SHL-06 │ Capabilities minimales (least privilege)              │
│ SHL-07 │ Mise à jour modèle = offline uniquement               │
│ SHL-08 │ DNS guard activé (anti-C2)                            │
│ SHL-09 │ Log audit = ring buffer non-bloquant                  │
│ SHL-10 │ IPC gate : capability vérifiée sur chaque message     │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Shield sans sandbox propre                                  │
│ ✗  Ré-implémenter XChaCha20 dans shield (appeler kernel)       │
│ ✗  Bloquer systématiquement sans score d'anomalie              │
│ ✗  Mise à jour modèle ML en Ring 0 runtime                     │
│ ✗  Watchdog absent (protection silencieuse si crash)           │
└────────────────────────────────────────────────────────────────┘
```

---
---
---

# 📋 ORDRE D'INITIALISATION GLOBAL (BOOT COMPLET)

```
SÉQUENCE DE BOOT EXO-OS — ORDRE STRICT

1.  arch::boot::early_init()                    # GDT minimal, paging identité
2.  arch::boot::parse_memory_map()              # E820 / UEFI map
3.  memory::physical::frame::emergency_pool::init()   # ← EN PREMIER ABSOLU
4.  memory::physical::allocator::bitmap::bootstrap()  # Allocateur minimal
5.  memory::physical::allocator::buddy::init()
6.  memory::heap::allocator::global::init()     # #[global_allocator] actif
7.  memory::utils::futex_table::init()
8.  arch::x86_64::gdt::init()
9.  arch::x86_64::idt::init()
10. arch::x86_64::tss::init_with_ist_stacks()   # IST #DF/#NMI/#MCE
11. arch::x86_64::apic::local_apic::init()
12. arch::x86_64::acpi::parser::init()
13. scheduler::core::init()
14. scheduler::fpu::save_restore::detect_xsave_size()
15. scheduler::timer::tick::init(HZ=1000)
16. scheduler::timer::hrtimer::init()
17. security::capability::init()                # TCB capabilities — avant process/
18. security::crypto::rng::init()               # CSPRNG (RDRAND + entropy)
19. process::core::registry::init()
20. process::state::wakeup::register_with_dma() # Enregistrer DmaWakeupHandler
21. memory::dma::iommu::init()                  # IOMMU (après PCI enum)
22. fs::core::vfs::init()
23. fs::ext4plus::mount_root()
24. ipc::core::init()
25. security::exploit_mitigations::kaslr::verify() # Vérifier KASLR actif
26. arch::x86_64::smp::start_aps()             # Démarrer APs SMP
27. memory::physical::frame::pool::init_percpu() # Per-CPU pools (après SMP)
28. memory::utils::oom_killer::start_thread()
29. process::lifecycle::create::spawn_pid1()    # init_server (PID 1)
30. # PID 1 démarre shield, drivers, services...
```

> ⚠️ **ERREUR CORRIGÉE** dans la séquence originale :
> Step 17 dans l'original : `security::capability::init()` était placé **après** `process::core::registry::init()` (step 19).
> C'est incorrect : `process/` dépend de `security/capability/` pour les vérifications de droits.
> `security::capability::init()` doit être initialisé AVANT `process/`.
> **Correction : security::capability::init() = step 17, process::core::registry::init() = step 19.**

---

# 📋 SYNTHÈSE DES RÈGLES TRANSVERSALES

```
┌─────────────────────────────────────────────────────────────────┐
│ RÈGLES TRANSVERSALES — applicables à TOUS les modules           │
├─────────────────────────────────────────────────────────────────┤
│ TRANS-01 │ Ordre des couches : memory(0)→scheduler(1)→          │
│            │ process(1.5)→ipc(2a)→fs(3)                         │
│            │ JAMAIS de dépendance remontante                     │
│ TRANS-02 │ Dépendances circulaires → trait abstrait             │
│            │ enregistré au boot (DmaWakeupHandler, ElfLoader...) │
│ TRANS-03 │ EmergencyPool initialisé EN PREMIER (step 3 boot)    │
│ TRANS-04 │ FutexTable = UNIQUE dans memory/utils/               │
│            │ Tous les modules délèguent                          │
│ TRANS-05 │ CapabilityVerify = UNIQUE dans security/capability/  │
│            │ Tous les modules délèguent                          │
│ TRANS-06 │ Lock ordering = ordre croissant des IDs              │
│            │ (CPU IDs, inode IDs...) — jamais inversé           │
│ TRANS-07 │ Hot path = zéro allocation, zéro sleep, zéro lock    │
│            │ Utiliser per-CPU pools, structures inline           │
│ TRANS-08 │ IRQ handlers = zéro allocation (per-CPU ou           │
│            │ EmergencyPool uniquement)                           │
│ TRANS-09 │ RAII partout : PreemptGuard, SpinLockGuard, etc.     │
│            │ Jamais disable/enable directs                       │
│ TRANS-10 │ Signal au retour userspace uniquement                 │
│            │ (pas depuis hot path scheduler)                     │
│ TRANS-11 │ TLB shootdown synchrone AVANT free_pages()           │
│ TRANS-12 │ DMA frame → DMA_PINNED jusqu'à ACK completion        │
│ TRANS-13 │ Pages IPC/SHM → NO_COW + SHM_PINNED obligatoires    │
│ TRANS-14 │ Context switch : sauvegarder r15 + MXCSR + x87 FCW  │
│ TRANS-15 │ CR3 switché DANS switch_asm (avant restauration regs) │
│ TRANS-16 │ Retpoline sur tous les appels indirects hot path      │
│ TRANS-17 │ Spectre v1 : array_index_nospec() sur accès tableaux │
│ TRANS-18 │ XChaCha20 sur tous canaux inter-domaines kernel       │
│ TRANS-19 │ IA kernel = lookup table .rodata ou EMA O(1)         │
│            │ Jamais inférence dynamique en Ring 0                │
│ TRANS-20 │ Panic pour état corrompu, Err() pour récupérable      │
│            │ RAII Drop pour cleanup multi-module                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 📋 CORRECTIONS APPORTÉES À DOC4-9

| # | Doc | Localisation | Erreur | Correction |
|---|---|---|---|---|
| 1 | DOC4 | `process/core/tcb.rs` | `dma_completion_result` absent du TCB mais référencé dans `wakeup.rs` | Ajouté `pub dma_completion_result: AtomicU8` — padding recalculé |
| 2 | DOC4 | `_pad2: [u8; 16]` | Padding incorrect si `dma_completion_result` ajouté | Corrigé en `[u8; 15]` |
| 3 | DOC5 | `ipc/stats/` | `mod.rs` manquant | Ajouté |
| 4 | DOC5 | `IPC-03` règle | `SHM_PINNED` non mentionné | Ajouté — les deux flags sont obligatoires |
| 5 | DOC7 | SEC-03 règle | `fs/ et ipc/ appellent security/capability/` | Corrigé : `ipc/` passe par `capability_bridge/`, `fs/` et `process/` appellent directement |
| 6 | Boot | Step 17 + 19 | `security::capability::init()` après `process::core::registry::init()` | Inversé — security/ doit être initialisé AVANT process/ |
| 7 | Boot | Commentaire | Erreur non documentée dans l'original | Ajouté encadré ⚠️ explicatif |
| 8 | TRANS-13 | Règle transversale | `NO_COW` seul mentionné | Complété : `NO_COW + SHM_PINNED` (cohérent avec IPC-03) |

---

*Documents 4 à 9 + Règles transversales — Exo-OS — v5 corrigé*
*Série complète: DOC1 (Arborescence) · DOC2 (Memory) · DOC3 (Scheduler) · DOC4-9 (Modules)*
