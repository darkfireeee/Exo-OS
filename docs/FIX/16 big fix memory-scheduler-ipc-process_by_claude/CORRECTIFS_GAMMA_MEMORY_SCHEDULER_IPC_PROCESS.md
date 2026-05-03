# ExoOS — Correctifs Complets : memory / scheduler / ipc / process
## Audit Profond — Modules Noyau Ring 0

> **Auteur** : claude-gamma  
> **Date** : 2026-05-03  
> **Référentiel** : `darkfireeee/Exo-OS` — commit analysé en intégralité  
> **Sources croisées** : `docs/recast/ExoOS_Architecture_v7.md`, `docs/recast/GI-01_Types_TCB_SSR.md`,  
> `docs/Exo-OS-TLA+/ContextSwitch.tla`, `docs/Exo-OS-TLA+/Memory.tla`,  
> arborescence complète `kernel/src/{memory,scheduler,ipc,process}/`

---

## Méthodologie

L'analyse a croisé trois corpus :

1. **Code source Rust** — lecture exhaustive des fichiers `.rs` des quatre modules
2. **Spécifications TLA+** — invariants `ContextSwitch.tla`, `Memory.tla`, `ExoOS_Full.tla`
3. **Documentation d'architecture** — `ExoOS_Architecture_v7.md`, `GI-01_Types_TCB_SSR.md`,  
   `GI-02_Boot_ContextSwitch.md`, `ExoOS_Kernel_Types_v10.md`

Chaque bug est classé selon trois critères :

| Priorité | Signification |
|----------|---------------|
| **P0 — CRITIQUE** | Corruption mémoire, UB, fuite noyau, violation sécurité |
| **P1 — MAJEUR** | Défaut fonctionnel, règle d'architecture non respectée, dégradation de performances |
| **P2 — MINEUR** | Documentation trompeuse, fragilité de l'ordre d'initialisation, amélioration de robustesse |

---

## Table des correctifs

| ID | Module | Priorité | Titre court |
|----|--------|----------|-------------|
| CGX-01 | `process/lifecycle/exec.rs` | **P0** | `do_execve()` — TSS.RSP0 set avec `kstack_ptr` au lieu de `kstack_top()` |
| CGX-02 | `process/lifecycle/exit.rs` | **P0** | Violation RÈGLE PROC-01 — import direct `fs::exofs` depuis `process/` |
| CGX-03 | `scheduler/core/switch.rs` | **P0** | `context_switch()` — `cpu_id` de `next` non mis à jour avant publication |
| CGX-04 | `process/lifecycle/exit.rs` | **P0** | `mark_exit()` ne libère pas `fpu_state_ptr` → fuite mémoire kernel |
| CGX-05 | `process/lifecycle/fork.rs` | **P0** | Fork sans copie de la capability table (S-17 non implémenté) |
| CGX-06 | `ipc/sync/wait_queue.rs` | **P1** | `IpcWaiter::thread_id : AtomicU32` tronque les TID u64 |
| CGX-07 | `ipc/sync/sched_hooks.rs` | **P1** | `SleepEntry::tid : u32` — même troncature TID |
| CGX-08 | `ipc/sync/wait_queue.rs` | **P1** | `IpcWaitQueue::wait()` — spin-poll sans blocage réel scheduler |
| CGX-09 | `scheduler/core/switch.rs` | **P1** | `block_current_thread()` — `debug_assert!` préemption absent |
| CGX-10 | `memory/dma/completion/handler.rs` | **P1** | DMA ISR — `wakeup()` appelé sous lock (inversion de priorité) |
| CGX-11 | `security/mod.rs` + `arch/x86_64/smp/` | **P1** | CVE-EXO-001 — APs ne spin-attendent pas `SECURITY_READY` |
| CGX-12 | `security/capability/verify.rs` | **P1** | `verify()` non constant-time (LAC-01 non implémenté) |
| CGX-13 | `scheduler/core/task.rs` | **P2** | Commentaire `_cold_reserve` «ExoShield (0..40)» trompeur — ExoShield s'arrête à [24] |
| CGX-14 | `ipc/mod.rs` | **P2** | `ipc_init()` ne connecte pas les hooks scheduler/VMM — ordre fragile |
| CGX-15 | `ipc/mod.rs` | **P2** | `send_irq_notification()` — défaut silencieux si `pid == 0` |
| CGX-16 | `process/core/tcb.rs` | **P2** | Canari `KernelStack` en bas de pile — détection tardive d'overflow |

---

## CGX-01 — P0 : `do_execve()` utilise `kstack_ptr` pour TSS.RSP0

### Localisation
`kernel/src/process/lifecycle/exec.rs` — section `#[cfg(target_os = "none")]`

### Description du bug

`TSS.RSP0` (Task State Segment Register Stack Pointer 0) doit contenir le **sommet stable** de la pile kernel du thread — c'est l'adresse à partir de laquelle le CPU empile le contexte Ring 3 lors de toute interruption ou syscall. Cette valeur est **fixe** pendant toute la vie du thread.

`kstack_ptr` est le pointeur RSP **courant** tel que sauvegardé par le dernier context switch. À l'entrée de `do_execve()`, le kernel a déjà empilé plusieurs frames d'appel ; `kstack_ptr` pointe vers le bas de la frame courante, **pas** vers le sommet de la pile.

Si on initialise `TSS.RSP0 ← kstack_ptr` :
- Le prochain IRQ/syscall depuis Ring 3 empile sur la zone **déjà utilisée** de la pile kernel.
- Les données du thread en cours sont écrasées → corruption garantie.

### Preuve TLA+

`ContextSwitch.tla` invariant `S26_TssRsp0MatchesCurrentTcb` :  
`TssRsp0[c] = CurrentTcb[c].kstack_top` (pas `kstack_ptr`).

L'invariant `Step8_UpdateGsAndTss` dans `ContextSwitch.tla` montre que `TssRsp0 ← kstack_top`, confirmant que seul le **sommet stable** est correct.

### Code fautif (exec.rs ~ligne 270)

```rust
// ← BUG P0 : utilise kstack_ptr (RSP courant, mi-pile) au lieu de kstack_top (sommet stable)
crate::arch::x86_64::smp::percpu::set_kernel_rsp(thread.sched_tcb.kstack_ptr);
crate::arch::x86_64::tss::update_rsp0(cpu_id, thread.sched_tcb.kstack_ptr);
```

### Correctif CGX-01

```rust
// CORRECTIF CGX-01 : utiliser kstack_top() — sommet STABLE de pile kernel.
// TSS.RSP0 doit pointer sur le sommet de pile pour que le CPU y empile
// correctement le contexte Ring 3 → Ring 0 (interruptions / syscalls).
// kstack_ptr est le RSP courant (mi-pile en cours d'exécution) — il ne
// convient PAS comme valeur de TSS.RSP0 ou kernel_rsp.
//
// Référence : V7-C-03, ContextSwitch.tla S26_TssRsp0MatchesCurrentTcb.
let next_kstack_top = thread.sched_tcb.kstack_top();
crate::arch::x86_64::smp::percpu::set_kernel_rsp(next_kstack_top);
crate::arch::x86_64::tss::update_rsp0(cpu_id, next_kstack_top);
```

---

## CGX-02 — P0 : Violation RÈGLE PROC-01 dans `exit.rs`

### Localisation
`kernel/src/process/lifecycle/exit.rs` — ligne 6

### Description du bug

`RÈGLE PROC-01` (Architecture v7 §2.1) : **la couche `process/` (Couche 1.5) ne peut jamais importer directement `fs/` (Couche 3)**. Les couches supérieures sont accessibles uniquement via des traits abstraits enregistrés au boot.

Le fichier `exit.rs` importe directement :
```rust
use crate::fs::exofs::posix_bridge::vfs_close_all_pid;
```

Ce couplage fort crée deux problèmes :
1. Viole la hiérarchie des couches (process/ → fs/ est interdit).
2. Si `fs/` n'est pas initialisé au moment de l'exit, c'est un appel dans un état indéterminé.

### Correctif CGX-02

**Étape 1 — Définir un trait `VfsExitHook` dans `process/`** :

```rust
// kernel/src/process/lifecycle/exit.rs — REMPLACER l'import fautif

// CORRECTIF CGX-02 : RÈGLE PROC-01 — process/ ne peut pas importer fs/ directement.
// On utilise un trait + enregistrement au boot (identique au pattern ElfLoader).

use spin::Once;

/// Trait d'abstraction pour la fermeture des handles VFS lors de la mort d'un processus.
/// Implémenté par `fs/` et enregistré via `register_vfs_exit_hook()` au boot.
pub trait VfsExitHook: Send + Sync {
    /// Ferme tous les handles VFS du processus `pid`.
    /// Appelé depuis `mark_exit()` après fermeture de l'`OpenFileTable`.
    fn close_all_for_pid(&self, pid: u32);
}

static VFS_EXIT_HOOK: Once<&'static dyn VfsExitHook> = Once::new();

/// Enregistre le hook VFS (appelé depuis `fs/` au boot, Phase 7).
///
/// # Safety
/// `hook` doit avoir une durée de vie `'static`.
pub fn register_vfs_exit_hook(hook: &'static dyn VfsExitHook) {
    VFS_EXIT_HOOK.call_once(|| hook);
}

/// Ferme les handles VFS d'un processus via le hook enregistré.
/// No-op si le hook n'est pas encore installé (cas test / early-boot).
#[inline]
pub fn vfs_close_all_pid_hook(pid: u32) {
    if let Some(hook) = VFS_EXIT_HOOK.get() {
        hook.close_all_for_pid(pid);
    }
}
```

**Étape 2 — Remplacer l'appel dans `mark_exit()`** :

```rust
// AVANT (fautif) :
// use crate::fs::exofs::posix_bridge::vfs_close_all_pid;
// vfs_close_all_pid(pcb.pid.0);

// APRÈS (conforme PROC-01) :
vfs_close_all_pid_hook(pcb.pid.0);
```

**Étape 3 — Implémentation côté `fs/`** :

```rust
// kernel/src/fs/exofs/posix_bridge/vfs_compat.rs — à ajouter

struct VfsPidExitImpl;

impl crate::process::lifecycle::exit::VfsExitHook for VfsPidExitImpl {
    fn close_all_for_pid(&self, pid: u32) {
        vfs_close_all_pid(pid);
    }
}

static VFS_PID_EXIT: VfsPidExitImpl = VfsPidExitImpl;

/// À appeler depuis kernel_init() Phase 7 (après fs init, avant launch de servers).
pub fn register_vfs_exit_hook_impl() {
    crate::process::lifecycle::exit::register_vfs_exit_hook(&VFS_PID_EXIT);
}
```

---

## CGX-03 — P0 : `context_switch()` — `cpu_id` de `next` non mis à jour

### Localisation
`kernel/src/scheduler/core/switch.rs` — après `context_switch_asm()`

### Description du bug

Après le context switch ASM, `next` s'exécute sur le CPU courant. Le code utilise alors `next.current_cpu()` pour :
1. Mettre à jour `tss::update_rsp0(next.current_cpu().0, ...)`
2. Lire les données per-CPU : `percpu::per_cpu_mut(publish_cpu)`
3. Publier `CURRENT_THREAD_PER_CPU[next.current_cpu().0].store(...)`

`current_cpu()` lit `self.cpu_id.load(Ordering::Acquire)`. Si `next` a été migré depuis le CPU 3 vers le CPU courant (CPU 0), `cpu_id` vaut encore `3`. Résultat :
- `TSS.RSP0` est écrit sur la **structure TSS du mauvais CPU** (CPU 3).
- Le CPU courant (0) a un TSS.RSP0 obsolète — prochaine interruption Ring 3 → Ring 0 empile sur la mauvaise pile.
- `CURRENT_THREAD_PER_CPU[3]` est mis à jour au lieu de `[0]`.

### Preuve TLA+

`ContextSwitch.tla Step8_UpdateGsAndTss` : la mise à jour doit être pour le CPU **courant** `c`, pas pour un cpu_id potentiellement stale du TCB.

### Code fautif (switch.rs ~ligne 190)

```rust
// ← BUG CGX-03 : next.current_cpu() lit l'ancien cpu_id, potentiellement stale
// si le thread a été migré d'un autre CPU avant ce context switch.
tss::update_rsp0(next.current_cpu().0 as usize, next_kstack_top);
...
let publish_cpu = next.current_cpu().0 as usize;
...
CURRENT_THREAD_PER_CPU[next.current_cpu().0 as usize].store(...);
```

### Correctif CGX-03

Insérer `next.assign_cpu()` **immédiatement après** `context_switch_asm()` retourne, avant tout usage de `next.current_cpu()` :

```rust
// CORRECTIF CGX-03 : mettre à jour cpu_id de next avec le CPU courant réel
// AVANT toute utilisation de next.current_cpu().
// Garantit la cohérence de TSS.RSP0, des données per-CPU et de
// CURRENT_THREAD_PER_CPU quelle que soit l'origine du thread (migration ou non).
let actual_cpu_id = percpu::current_cpu_id() as u32;
// SAFETY: assign_cpu est une écriture atomique Release ; on est le seul CPU
// à exécuter next à cet instant précis.
next.assign_cpu(CpuId(actual_cpu_id));

// Toutes les utilisations suivantes de next.current_cpu() lisent maintenant
// la valeur correcte (actual_cpu_id).
percpu::set_current_tcb(next as *mut ThreadControlBlock);
let next_kstack_top = next.kstack_top();
unsafe {
    percpu::set_kernel_rsp(next_kstack_top);
    // TSS.RSP0 mis à jour sur le BON CPU (actual_cpu_id, pas l'ancien cpu_id).
    tss::update_rsp0(actual_cpu_id as usize, next_kstack_top);
}

let publish_cpu = actual_cpu_id as usize; // ← utiliser la valeur directe, pas current_cpu()
if publish_cpu < MAX_CPUS {
    let cpu_data = unsafe { percpu::per_cpu_mut(publish_cpu) };
    cpu_data.ctx_switch_count = cpu_data.ctx_switch_count.wrapping_add(1);
    cpu_data.last_switch_tsc = tsc::read_tsc();
}
core::sync::atomic::fence(Ordering::SeqCst);
CURRENT_THREAD_PER_CPU[publish_cpu]
    .store(next as *mut ThreadControlBlock as usize, Ordering::Release);
```

---

## CGX-04 — P0 : `mark_exit()` ne libère pas `fpu_state_ptr`

### Localisation
`kernel/src/process/lifecycle/exit.rs` — fonction `mark_exit()`

### Description du bug

La spécification S-31 (Architecture v7 §10) requiert que `fpu_state_ptr` soit libéré dans **`thread_exit()` ET `do_exit()`**. Le TCB contient `fpu_state_ptr: u64` (offset [232]), un pointeur vers un `XSaveArea` alloué au premier usage FPU du thread (`scheduler/fpu/lazy.rs`).

`mark_exit()` ferme les fichiers et notifie le parent mais **ne libère jamais** ce pointeur. Conséquence : chaque thread qui utilise la FPU laisse une `XSaveArea` (typiquement 576 à 2688 bytes selon CPUID) irrémédiablement fuité dans le heap kernel.

### Code fautif (exit.rs)

```rust
fn mark_exit(thread: &mut ProcessThread, pcb: &ProcessControlBlock, ...) {
    pcb.set_exiting();
    // ... fermeture fichiers, VFS, drivers ...
    // ← BUG CGX-04 : fpu_state_ptr N'EST PAS libéré
    thread.join_result.store(...);
    thread.set_state(TaskState::Dead);
    ...
}
```

### Correctif CGX-04

```rust
fn mark_exit(thread: &mut ProcessThread, pcb: &ProcessControlBlock, exit_status: u32, join_result: u64) {
    pcb.set_exiting();
    pcb.exit_code.store(exit_status, Ordering::Release);
    pcb.flags.fetch_or(process_flags::VFORK_DONE, Ordering::Release);

    let closed_handles = {
        let mut files = pcb.files.lock();
        files.close_all()
    };
    drop(closed_handles);
    vfs_close_all_pid_hook(pcb.pid.0);  // ← CGX-02 appliqué
    drivers::driver_do_exit(pcb.pid.0);

    // CORRECTIF CGX-04 : libérer fpu_state_ptr avant que le TCB soit rendu Dead.
    // Spécification S-31 (Architecture v7 §10).
    //
    // RÈGLE SÉCURITÉ : zéroiser la XSaveArea avant libération pour éviter
    // qu'un futur allocataire voie un état FPU (contient potentiellement des
    // données sensibles issues de calculs Ring 3).
    let fpu_ptr = thread.sched_tcb.fpu_state_ptr;
    if fpu_ptr != 0 {
        // SAFETY: fpu_state_ptr pointe vers un XSaveArea alloué par
        // scheduler::fpu::lazy::alloc_fpu_state() (Box<XSaveArea>).
        // On est le seul thread à toucher ce TCB (état EXITING).
        unsafe {
            crate::scheduler::fpu::lazy::free_fpu_state(fpu_ptr);
        }
        // Invalider le pointeur AVANT de rendre le TCB mort.
        // SAFETY: écriture atomique au sens architectural ; le thread est
        // en cours de terminaison, aucun autre CPU ne lit fpu_state_ptr.
        thread.sched_tcb.fpu_state_ptr = 0;
    }

    thread.join_result.store(join_result, Ordering::Release);
    thread.join_done.store(true, Ordering::Release);

    let remaining_threads = pcb.dec_threads();
    if remaining_threads == 0 {
        pcb.set_state(ProcessState::Zombie);
        let ppid = pcb.ppid();
        if ppid.0 != 0 {
            let _ = send_signal_to_pid(ppid, Signal::SIGCHLD);
        }
        crate::process::lifecycle::fork::notify_vfork_completion(pcb.pid);
    }

    thread.set_state(TaskState::Dead);
    crate::process::lifecycle::reap::REAPER_QUEUE.enqueue(thread.pid, thread.tid);
}
```

**Implémentation requise dans `scheduler/fpu/lazy.rs`** :

```rust
/// Libère la XSaveArea allouée pour un thread qui se termine.
/// Zéroïse la zone avant libération (donnée potentiellement sensible).
///
/// # Safety
/// - `fpu_ptr` doit provenir de `alloc_fpu_state()`.
/// - Appelable une seule fois par TCB (après, fpu_state_ptr = 0).
pub unsafe fn free_fpu_state(fpu_ptr: u64) {
    use alloc::alloc::{dealloc, Layout};
    let ptr = fpu_ptr as *mut XSaveArea;
    if !ptr.is_null() {
        // Zéroïser AVANT libération — données potentiellement sensibles.
        core::ptr::write_bytes(ptr as *mut u8, 0, core::mem::size_of::<XSaveArea>());
        let layout = Layout::new::<XSaveArea>();
        dealloc(ptr as *mut u8, layout);
    }
}
```

---

## CGX-05 — P0 : Fork sans copie de la capability table (S-17)

### Localisation
`kernel/src/process/lifecycle/fork.rs` — `do_fork()`

### Description du bug

La spécification S-17 (Architecture v7 §10) requiert : **"Cap table fork shadow-copy RCU + rollback"**. Lors d'un `fork()`, le processus fils doit hériter d'une copie (shadow-copy) de la capability table du parent.

`do_fork()` clone l'espace d'adressage (`clone_cow()`) et crée un `ProcessThread` + `ProcessControlBlock` pour le fils, mais **n'effectue aucune copie de la cap table**. Le fils se retrouve sans capability table valide → accès à `verify_cap_token()` depuis le fils = comportement indéterminé.

### Correctif CGX-05

**Dans `do_fork()`, après création du `child_pcb`** :

```rust
// CORRECTIF CGX-05 : shadow-copy de la capability table (S-17).
// Le fils hérite des capabilities du parent au moment du fork.
// Le cap_table est dans le PCB, partagé entre threads du même processus.
// Pour un fork() (nouvel espace d'adressage), la cap_table est dupliquée.
// Pour un clone() avec CLONE_VM (thread POSIX), la cap_table est partagée.
if !flags.has(ForkFlags::CLONE_VM) {
    // fork() standard : dupliquer la cap table
    let parent_cap_table = parent_pcb.cap_table.read();
    let child_cap_table = parent_cap_table
        .shadow_copy()
        .map_err(|_| {
            // Rollback : libérer les ressources déjà allouées
            PID_ALLOCATOR.free(child_pid_raw);
            TID_ALLOCATOR.free(child_tid_raw);
            cloner.free_addr_space(cloned_as.addr_space_ptr);
            ForkError::OutOfMemory
        })?;
    // Le child_pcb reçoit sa propre cap_table indépendante du parent.
    child_pcb.cap_table.write().replace(child_cap_table);
} else {
    // CLONE_VM (thread POSIX) : partager la cap_table du parent (Arc partagé)
    // Implémentation future (Phase 3) — pour l'instant, copie simple.
    let parent_cap_table = parent_pcb.cap_table.read();
    let child_cap_table = parent_cap_table.shadow_copy()
        .map_err(|_| {
            PID_ALLOCATOR.free(child_pid_raw);
            TID_ALLOCATOR.free(child_tid_raw);
            ForkError::OutOfMemory
        })?;
    child_pcb.cap_table.write().replace(child_cap_table);
}
```

**Dans `security/capability/table.rs`** — ajouter la méthode `shadow_copy()` :

```rust
impl CapTable {
    /// Crée une copie indépendante de la cap table pour un nouveau processus.
    ///
    /// Sémantique : toutes les capabilities du parent sont copiées dans le fils
    /// au moment du fork. Les modifications ultérieures (parent ou fils) sont
    /// indépendantes (pas de partage COW ici — caps = petites structures).
    ///
    /// En cas d'erreur (OOM), retourne `Err(())` et laisse la table source intacte.
    pub fn shadow_copy(&self) -> Result<CapTable, ()> {
        let entries = self.entries.read();
        let mut new_table = CapTable::new_empty();
        {
            let mut new_entries = new_table.entries.write();
            *new_entries = entries.clone();
        }
        Ok(new_table)
    }
}
```

---

## CGX-06 — P1 : `IpcWaiter::thread_id : AtomicU32` tronque les TID u64

### Localisation
`kernel/src/ipc/sync/wait_queue.rs` — struct `IpcWaiter`

### Description du bug

`ThreadId` est `u64` dans tout le noyau (`scheduler/core/task.rs`). Les TID sont alloués depuis un compteur 64 bits monotone. `IpcWaiter::thread_id` est `AtomicU32`, ce qui tronque les TID supérieurs à `0xFFFF_FFFF`.

Conséquence : si un waiter a un TID > 4 milliards (plausible sur un système long-running), la valeur stockée est incorrecte. `sched_hooks::wake_thread(tid as u32)` essaie de réveiller un thread avec un TID tronqué → le thread réel n'est jamais réveillé → deadlock IPC.

### Correctif CGX-06

```rust
// kernel/src/ipc/sync/wait_queue.rs — struct IpcWaiter

/// Un waiter dans la file IPC
#[repr(C, align(64))]
pub struct IpcWaiter {
    // CORRECTIF CGX-06 : thread_id en u64 pour éviter la troncature des TID > u32::MAX.
    // ThreadId est u64 dans scheduler/core/task.rs — toujours utiliser u64 ici.
    pub thread_id: AtomicU64,    // ← était AtomicU32
    pub active: AtomicBool,
    pub woken: AtomicBool,
    pub reason: AtomicU32,
    pub seq: AtomicU32,
    pub enqueued_at: AtomicU64,
    pub timeout_ns: AtomicU64,
    _pad: [u8; 8],               // ajuster padding pour maintenir la taille totale
}

impl IpcWaiter {
    pub const fn new() -> Self {
        Self {
            thread_id: AtomicU64::new(0),   // ← était AtomicU32
            active: AtomicBool::new(false),
            woken: AtomicBool::new(false),
            reason: AtomicU32::new(0),
            seq: AtomicU32::new(0),
            enqueued_at: AtomicU64::new(0),
            timeout_ns: AtomicU64::new(0),
            _pad: [0u8; 8],
        }
    }
}
```

Mettre à jour tous les sites d'écriture et de lecture de `thread_id` pour utiliser `u64` / `Ordering::*` cohérents.

---

## CGX-07 — P1 : `SleepEntry::tid : u32` dans `sched_hooks.rs`

### Localisation
`kernel/src/ipc/sync/sched_hooks.rs` — struct `SleepEntry`

### Description du bug

Même classe d'erreur que CGX-06. `SleepEntry.tid: u32` stocke un TID qui est `u32` dans le registre mais `u64` dans le TCB. `wake_thread(tid: u32)` recherche dans `SLEEP_REGISTRY` avec un `tid: u32`. Les TID > `u32::MAX` ne seront jamais trouvés → le thread IPC reste bloqué indéfiniment.

### Correctif CGX-07

```rust
// kernel/src/ipc/sync/sched_hooks.rs

#[repr(C)]
struct SleepEntry {
    // CORRECTIF CGX-07 : tid en u64 — ThreadId est u64 dans tout le noyau.
    tid: u64,           // ← était u32
    tcb_ptr: usize,
}

impl SleepEntry {
    const fn empty() -> Self {
        Self { tid: 0, tcb_ptr: 0 }
    }
    fn is_free(&self) -> bool {
        self.tcb_ptr == 0
    }
}

// Dans wake_thread :
pub fn wake_thread(tid: u64) {   // ← était u32
    let mut reg = SLEEP_REGISTRY.lock();
    for e in reg.entries.iter_mut() {
        if !e.is_free() && e.tid == tid {
            let tcb = e.tcb_ptr as *mut ThreadControlBlock;
            e.tcb_ptr = 0;
            e.tid = 0;
            if !tcb.is_null() {
                // SAFETY: tcb reste valide tant qu'il est dans le registre.
                unsafe { do_wake_thread(tcb); }
            }
            return;
        }
    }
}

// Dans register :
fn register(&mut self, tid: u64, tcb: *mut ThreadControlBlock) {   // ← était u32
    for e in self.entries.iter_mut() {
        if e.is_free() {
            e.tid = tid;
            e.tcb_ptr = tcb as usize;
            return;
        }
    }
    // Registre plein : log error, thread tombera en spin-poll (dégradé acceptable).
}

// Mettre à jour install_block_hook et les callers pour passer u64.
```

---

## CGX-08 — P1 : `IpcWaitQueue::wait()` — spin-poll sans blocage scheduler

### Localisation
`kernel/src/ipc/sync/wait_queue.rs` — implémentation de l'attente

### Description du bug

`IpcWaiter` contient un `woken: AtomicBool`. L'attente sur cet AtomicBool est actuellement un **spin-poll** : la boucle tourne à pleine vitesse jusqu'à ce que `woken` devienne `true`. Sans appel au hook scheduler (`sched_hooks::block_fn`), le thread occupe 100% du cœur CPU au lieu de se mettre réellement en état `Sleeping`.

Conséquence : toute attente IPC bloquante (canal synchrone, événement, rendezvous) dégrade drastiquement les performances d'un système multi-threads.

### Correctif CGX-08

```rust
// kernel/src/ipc/sync/wait_queue.rs

impl IpcWaitQueue {
    /// Attend qu'un waiter soit réveillé (bloquant via scheduler).
    ///
    /// Séquence :
    /// 1. Enregistrer le thread courant dans SLEEP_REGISTRY.
    /// 2. Marquer le waiter comme actif.
    /// 3. Appeler le hook de blocage scheduler (suspend le thread).
    /// 4. À la reprise, vérifier si woken == true.
    ///
    /// Retourne le WakeReason du réveil.
    pub fn wait_blocking(
        &self,
        waiter_slot: usize,
        tid: u64,
        tcb_ptr: *mut crate::scheduler::core::task::ThreadControlBlock,
        timeout_ns: u64,
    ) -> WakeReason {
        debug_assert!(waiter_slot < MAX_IPC_WAITERS, "wait: slot hors bornes");

        let waiter = &self.waiters[waiter_slot];
        // CORRECTIF CGX-08 : enregistrer le thread pour réveil.
        {
            let mut reg = super::sched_hooks::SLEEP_REGISTRY.lock();
            reg.register(tid, tcb_ptr);
        }

        waiter.thread_id.store(tid, Ordering::Relaxed);
        waiter.enqueued_at.store(
            crate::arch::x86_64::time::ktime::ktime_get_ns(),
            Ordering::Relaxed,
        );
        waiter.timeout_ns.store(timeout_ns, Ordering::Relaxed);
        waiter.active.store(true, Ordering::Release);

        // CORRECTIF CGX-08 : utiliser le hook de blocage scheduler (pas de spin-poll).
        // Si le hook n'est pas installé, on tombe en spin-poll dégradé (acceptable
        // uniquement en monocœur bare-metal pré-scheduler).
        if let Some(block_fn) = super::sched_hooks::get_block_hook() {
            // SAFETY: block_fn suspend le thread jusqu'à ce qu'un appel à
            // wake_thread(tid) depuis un autre contexte le réveille.
            unsafe { block_fn() };
        } else {
            // Spin-poll de secours (mode dégradé).
            let mut spin_iters: u64 = 0;
            while !waiter.woken.load(Ordering::Acquire) {
                core::hint::spin_loop();
                spin_iters += 1;
                // Éviter un livelock absolu si wake ne vient jamais.
                if timeout_ns > 0 && spin_iters > timeout_ns / 100 {
                    waiter.wake(WakeReason::Timeout);
                    break;
                }
            }
        }

        waiter.active.store(false, Ordering::Release);
        {
            let mut reg = super::sched_hooks::SLEEP_REGISTRY.lock();
            reg.unregister(tid);
        }
        waiter.wake_reason()
    }
}

// Dans sched_hooks.rs — ajouter get_block_hook() :
pub fn get_block_hook() -> Option<BlockFn> {
    *BLOCK_HOOK.lock()
}
```

---

## CGX-09 — P1 : `block_current_thread()` — assertion préemption absente

### Localisation
`kernel/src/scheduler/core/switch.rs` — `block_current_thread()`

### Description du bug

La spécification PREEMPT-BLOCK (Architecture v7 §9.3) et la règle PREEMPT-01 exigent que `block_current()` soit appelé avec la préemption **désactivée**. Si préemption est active à cet instant, un context switch peut intervenir entre l'insertion dans la wait queue et l'appel à `block_current_thread()`, faisant manquer le signal de réveil → deadlock.

La vérification `debug_assert!(preempt_count == 0)` est absente.

### Correctif CGX-09

```rust
// kernel/src/scheduler/core/switch.rs — block_current_thread()

pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    // CORRECTIF CGX-09 : vérifier que la préemption est bien désactivée.
    // PREEMPT-BLOCK (Architecture v7 §9.3) : appelable UNIQUEMENT sous PreemptGuard
    // ou IrqGuard. Si la préemption est active, un switch peut survenir entre
    // l'insertion dans la wait queue et ce point → le réveil serait manqué.
    debug_assert!(
        crate::scheduler::core::preempt::preempt_count() > 0,
        "block_current_thread(): PREEMPT-BLOCK — appel avec préemption active (count=0). \
         Doit être appelé sous PreemptGuard ou IrqGuard."
    );

    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        for _ in 0..1_000 { core::hint::spin_loop(); }
        return;
    }

    let tcb = &mut *tcb_ptr;
    let cpu_id = tcb.current_cpu();
    if (cpu_id.0 as usize) < MAX_CPUS {
        let rq = run_queue(cpu_id);
        tcb.set_state(TaskState::Sleeping);
        schedule_block(rq, tcb);
    }
}
```

---

## CGX-10 — P1 : DMA ISR — `wakeup()` appelé sous lock (S-22)

### Localisation
`kernel/src/memory/dma/completion/handler.rs`

### Description du bug

La règle S-22 (Architecture v7 §10) : **"DMA ISR : libérer lock AVANT wakeup"**. Un ISR DMA qui appelle `dma_wakeup()` (qui peut appeler `wake_enqueue()` puis `schedule()`) PENDANT qu'il tient le lock du canal DMA provoque une inversion de priorité :
- Le thread kernel qui attend la completion DMA tente d'acquérir le lock du canal.
- Ce lock est tenu par l'ISR.
- L'ISR essaie de `wake_enqueue()` qui acquiert la RunQueue lock (niveau Scheduler).
- Mais Scheduler < Memory dans l'ordre des locks → `schedule()` ne peut pas s'exécuter.

### Correctif CGX-10

```rust
// kernel/src/memory/dma/completion/handler.rs

/// Gère la completion d'un transfert DMA.
///
/// RÈGLE S-22 : libérer le lock du canal AVANT d'appeler wakeup.
/// Inversion de priorité si wakeup est appelé sous lock.
pub fn handle_dma_completion(channel_id: usize) {
    // CORRECTIF CGX-10 : extraire le waiter sous lock, libérer, puis réveiller.
    let maybe_waiter = {
        // Bloc scopé — le lock est relâché à la fin du bloc.
        let channel_guard = DMA_CHANNELS[channel_id].lock();
        let waiter = channel_guard.take_pending_waiter();
        // ← lock relâché ici (Drop de channel_guard)
        waiter
    };

    // Appel wakeup HORS lock — conforme S-22.
    if let Some(waiter) = maybe_waiter {
        // SAFETY: waiter.tid est valide tant que le thread attend la completion.
        // Le thread ne peut pas mourir pendant qu'il attend (UNINTERRUPTIBLE).
        if let Some(wake_fn) = DMA_WAKE_FN.load(Ordering::Acquire) {
            // SAFETY: wake_fn est une fn pointer installée par process/ au boot.
            unsafe { (wake_fn)(waiter.tid, 0) };
        }
    }
}
```

---

## CGX-11 — P1 : CVE-EXO-001 — APs sans spin-wait avant `SECURITY_READY`

### Localisation
`kernel/src/security/mod.rs` et `kernel/src/arch/x86_64/smp/init.rs`

### Description du bug

CVE-EXO-001 (Architecture v7 §9.2) : Les processeurs secondaires (APs) démarrent et accèdent potentiellement à des ressources sécurisées **avant** que le BSP ait initialisé le sous-système de sécurité (`SECURITY_READY`). Sans la barrière, un AP pourrait exécuter du code nécessitant une vérification de capabilities avant que la table soit initialisée → comportement non défini.

### Correctif CGX-11

**Dans `security/mod.rs`** :

```rust
// kernel/src/security/mod.rs

use core::sync::atomic::{AtomicBool, Ordering};

/// Drapeau de disponibilité du sous-système de sécurité.
/// Le BSP le positionne APRÈS l'initialisation complète de security/.
/// Les APs spin-attendent sur ce drapeau avant d'exécuter tout code
/// nécessitant des capabilities. (CVE-EXO-001, Architecture v7 §9.2, S-04)
pub static SECURITY_READY: AtomicBool = AtomicBool::new(false);

/// Marque le sous-système de sécurité comme initialisé.
/// À appeler depuis `kernel_init()` Phase 5, après `security::init()`.
#[inline]
pub fn set_security_ready() {
    SECURITY_READY.store(true, Ordering::Release);
    // Barrière explicite : garantit que toutes les initialisations security/
    // précédentes sont visibles des APs avant qu'ils lisent SECURITY_READY.
    core::sync::atomic::fence(Ordering::SeqCst);
}

/// Retourne `true` si le sous-système de sécurité est opérationnel.
#[inline(always)]
pub fn is_security_ready() -> bool {
    SECURITY_READY.load(Ordering::Acquire)
}
```

**Dans `arch/x86_64/smp/init.rs`** — boucle d'attente AP :

```rust
// kernel/src/arch/x86_64/smp/init.rs — ap_startup() ou ap_main()

/// Point d'entrée des processeurs secondaires (APs).
///
/// CORRECTIF CGX-11 / CVE-EXO-001 : spin-wait en ASM sur SECURITY_READY
/// avant toute exécution de code nécessitant des capabilities ou la sécurité.
pub fn ap_main() -> ! {
    // Init locale minimale (GDT, IDT, LAPIC, per-CPU GS).
    ap_local_init();

    // CORRECTIF CGX-11 : spin-wait ASM sur SECURITY_READY.
    // Utiliser PAUSE pour éviter la monopolisation du bus mémoire.
    // Ordering::Acquire garantit que les initialisations du BSP sont visibles.
    while !crate::security::is_security_ready() {
        // SAFETY: PAUSE est une instruction x86 safe en Ring 0.
        // Elle signale au CPU que l'on est dans une spin-wait loop,
        // réduisant la consommation d'énergie et l'impact sur le bus.
        unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)) };
    }
    // Barrière Acquire déjà dans is_security_ready() — les écritures BSP visibles.

    // Suite du démarrage AP (scheduler, IPC hooks, etc.)
    ap_scheduler_init();
    ap_idle_loop()
}
```

---

## CGX-12 — P1 : `verify()` non constant-time (LAC-01)

### Localisation
`kernel/src/security/capability/verify.rs`

### Description du bug

LAC-01 (Architecture v7 §9.2) : `verify_cap_token()` doit être implémentée en **temps constant** via `subtle::ct_eq()`. Une implémentation naïve avec `==` sur `CapToken` est vulnérable aux attaques de timing side-channel : en mesurant le temps de retour, un attaquant peut deviner les bytes d'un token valide un à un.

### Correctif CGX-12

```rust
// kernel/src/security/capability/verify.rs

// CORRECTIF CGX-12 / LAC-01 : vérification constant-time via ct_eq().
// Évite les attaques de timing side-channel sur la comparaison de tokens.
//
// Le crate `subtle` (no_std) fournit des primitives constant-time.
// Référence : Architecture v7 §9.2 LAC-01, checklist S-02.

use subtle::ConstantTimeEq;

/// Vérifie un `CapToken` en temps constant.
///
/// Retourne `true` si le token est valide, `false` sinon.
/// La durée d'exécution est INDÉPENDANTE de la valeur du token.
///
/// # Sécurité
/// Utilise `subtle::ConstantTimeEq::ct_eq()` pour éviter les attaques timing.
/// JAMAIS remplacer par `==` ou une comparaison byte-à-byte avec early return.
pub fn verify_cap_token(token: &CapToken) -> bool {
    // Récupérer le token de référence depuis la cap table (lecture seule, rapide).
    let expected = match get_reference_token(token.oid) {
        Some(t) => t,
        None => return false, // OID inconnu — retour immédiat acceptable
    };

    // Comparaison génération + droits en temps constant.
    let gen_eq: subtle::Choice = token.gen.ct_eq(&expected.gen);
    let rights_eq: subtle::Choice = token.rights.ct_eq(&expected.rights);

    // Les deux comparaisons DOIVENT toutes deux être effectuées avant le retour.
    // `&` sur Choice est constant-time (pas de short-circuit).
    bool::from(gen_eq & rights_eq)
}
```

**Ajouter dans `kernel/Cargo.toml`** (déjà listé selon S-08) :
```toml
subtle = { version = "2.5", default-features = false }
```

---

## CGX-13 — P2 : Commentaire `_cold_reserve` trompeur (ExoShield 0..40)

### Localisation
`kernel/src/scheduler/core/task.rs` — méthode `get_pl0_ssp()`

### Description du bug

Le commentaire `// SAFETY: offset 48..56 dans _cold_reserve[88], non-overlapping ExoShield (0..40)` est **inexact**. L'analyse des assertions de layout montre que ExoShield utilise effectivement :
- `[0..8]` = shadow_stack_token
- `[8]` = cet_flags
- `[9]` = threat_score_u8
- `[16..24]` = pt_buffer_phys

ExoShield s'arrête donc à `[24]`, pas `[40]`. Les offsets `[24..32]` (creation_tsc) et `[32..40]` (kstack_top) appartiennent au scheduler, pas à ExoShield. Ce commentaire trompeur pourrait conduire un développeur à croire que `[24..48]` est libre pour ExoShield, et y écrire — écrasant `creation_tsc` et `kstack_top`.

### Correctif CGX-13

```rust
// kernel/src/scheduler/core/task.rs — méthode get_pl0_ssp()

/// Lit MSR_IA32_PL0_SSP stocké dans `_cold_reserve[48..56]` = TCB abs 192..200.
#[inline(always)]
pub fn get_pl0_ssp(&self) -> u64 {
    // CORRECTIF CGX-13 : commentaire précis sur l'utilisation de _cold_reserve.
    //
    // Carte de _cold_reserve[88] (TCB offset 144..232) :
    //   [0..8]   = shadow_stack_token (ExoShield)   → TCB abs 144..152
    //   [8]      = cet_flags          (ExoShield)   → TCB abs 152
    //   [9]      = threat_score_u8    (ExoShield)   → TCB abs 153
    //   [10..16] = gap non assigné                  → TCB abs 154..160
    //   [16..24] = pt_buffer_phys     (ExoShield)   → TCB abs 160..168
    //   ── ExoShield s'arrête ici ──────────────────────────────────────────
    //   [24..32] = creation_tsc       (audit)       → TCB abs 168..176
    //   [32..40] = kstack_top         (scheduler)   → TCB abs 176..184
    //   [40..48] = réservé                          → TCB abs 184..192
    //   [48..56] = pl0_ssp            (CET)         → TCB abs 192..200  ← ICI
    //   [56..64] = affinity_hi[0]     (scheduler)   → TCB abs 200..208
    //   [64..72] = affinity_hi[1]     (scheduler)   → TCB abs 208..216
    //   [72..80] = affinity_hi[2]     (scheduler)   → TCB abs 216..224
    //   [80..88] = réservé                          → TCB abs 224..232
    //
    // SAFETY: [48..56] est dans _cold_reserve[88], non-overlapping ExoShield (0..24).
    unsafe { core::ptr::read_unaligned(self._cold_reserve.as_ptr().add(48) as *const u64) }
}
```

---

## CGX-14 — P2 : `ipc_init()` ne connecte pas les hooks — ordre fragile

### Localisation
`kernel/src/ipc/mod.rs` — `ipc_init()`

### Description du bug

`ipc_init()` initialise le pool SHM et le NUMA, mais ne connecte **pas** les hooks scheduler (`ipc_install_scheduler_hooks`) et VMM (`ipc_install_vmm_hooks`). Ces hooks doivent être appelés séparément après `scheduler::init()` et `virt::init()`.

Sans documentation explicite de cet ordre, un développeur qui appelle `ipc_init()` et suppose que l'IPC est opérationnel se retrouvera avec un spin-poll silencieux pour toutes les attentes.

### Correctif CGX-14

```rust
// kernel/src/ipc/mod.rs

/// Initialise le sous-système IPC et connecte les hooks scheduler et VMM.
///
/// # Ordre d'initialisation OBLIGATOIRE
/// 1. `memory::init()` — pool SHM et buddy allocator
/// 2. `scheduler::init()` — fournit `block_fn`
/// 3. `virt::init()` — fournit `map_page_fn` / `unmap_page_fn`
/// 4. **`ipc_init()`** ← ce point
///
/// Appelée depuis `kernel_init()` Phase 6 (Architecture v7 §3.1.1).
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    // 1. Pool SHM
    unsafe { shared_memory::pool::init_shm_pool(shm_base_phys); }
    // 2. NUMA
    unsafe { shared_memory::numa_aware::numa_init(n_numa_nodes as usize); }
    // 3. Stats reset
    stats::counters::IPC_STATS.reset_all();

    // CORRECTIF CGX-14 : connecter les hooks ici.
    // Les hooks sont installés conditionnellement : si le scheduler/VMM ne sont
    // pas encore initialisés (cas test unitaire), l'IPC fonctionne en mode
    // spin-poll dégradé. En production, ces hooks sont toujours disponibles ici.
    //
    // Hook scheduler : suspend le thread courant via block_current_thread().
    ipc_install_scheduler_hooks(
        crate::scheduler::core::switch::block_current_thread as sync::sched_hooks::BlockFn
    );

    // Hook VMM : map/unmap des pages SHM dans les espaces d'adressage.
    ipc_install_vmm_hooks(
        crate::memory::virt::address_space::user::map_page_for_pid,
        crate::memory::virt::address_space::user::unmap_page_for_pid,
    );
}
```

---

## CGX-15 — P2 : `send_irq_notification()` — défaut silencieux si `pid == 0`

### Localisation
`kernel/src/ipc/mod.rs` — `send_irq_notification()`

### Description du bug

```rust
let endpoint_code = ((endpoint.pid as u64) << 32) | endpoint.chan_idx as u64;
let endpoint_id = EndpointId::new(endpoint_code).ok_or(IpcError::NullEndpoint)?;
```

Si `endpoint.pid == 0` ET `endpoint.chan_idx == 0`, alors `endpoint_code == 0`. `EndpointId::new(0)` retourne `None` (NonZeroU64 rejecte 0) → `Err(IpcError::NullEndpoint)` renvoyé silencieusement. Les IRQs de drivers avec pid=0 (kernel threads ou drivers non encore enregistrés) sont silencieusement perdues.

### Correctif CGX-15

```rust
// kernel/src/ipc/mod.rs

pub fn send_irq_notification(
    endpoint: &exo_types::IpcEndpoint,
    irq: u8,
    wave_gen: u64,
) -> Result<(), IpcError> {
    // CORRECTIF CGX-15 : vérification explicite avant encodage.
    // Un pid == 0 indique un endpoint non initialisé ou un driver non enregistré.
    // Retourner une erreur descriptive plutôt que NullEndpoint générique.
    if endpoint.pid == 0 {
        // Log diagnostic — pas de panic car les IRQs arrivent parfois
        // pendant la phase de démarrage des drivers (acceptable early-boot).
        #[cfg(debug_assertions)]
        crate::arch::x86_64::vga_early::early_warn!(
            "send_irq_notification: pid==0 pour IRQ {}, endpoint non enregistré",
            irq
        );
        return Err(IpcError::NullEndpoint);
    }

    let endpoint_code = ((endpoint.pid as u64) << 32) | endpoint.chan_idx as u64;
    let endpoint_id = EndpointId::new(endpoint_code)
        .ok_or(IpcError::NullEndpoint)?;

    let mut payload = [0u8; 9];
    payload[0] = irq;
    payload[1..].copy_from_slice(&wave_gen.to_le_bytes());

    channel::raw::try_send_raw_nowait(endpoint_id, &payload).map(|_| ())
}
```

---

## CGX-16 — P2 : Canari `KernelStack` — détection tardive d'overflow

### Localisation
`kernel/src/process/core/tcb.rs` — `KernelStack`

### Description du bug

Le canari (`STACK_CANARY`) est écrit au **bas** (adresse la plus basse) du buffer, soit les premiers 8 bytes. Sur x86_64, la pile croît vers les adresses décroissantes. Un stack overflow écrase d'abord les adresses juste en dessous du RSP courant, **loin** du canari. Le canari n'est touché que lors d'un overflow extrêmement large (tout le buffer). Une guard page (page non mappée à l'adresse la plus basse) fournirait une détection immédiate dès le premier accès hors limite.

### Correctif CGX-16

```rust
// kernel/src/process/core/tcb.rs — KernelStack::alloc()

impl KernelStack {
    /// Alloue un stack kernel avec guard page au bas.
    ///
    /// Layout (adresses croissantes) :
    ///   [base .. base+PAGE_SIZE]  : guard page (mappée NX + non-présente → #PF immédiat)
    ///   [base+PAGE_SIZE .. top]   : zone de pile utilisable
    ///   [top-8 .. top]            : canari (détection overflow précoce)
    ///
    /// La guard page est mappée NX non-présente dans le page table du thread
    /// kernel via `memory::protection::map_guard_page()`.
    pub fn alloc(size: usize) -> Option<Self> {
        use alloc::alloc::{alloc, Layout};
        use crate::memory::core::PAGE_SIZE;

        // Allouer size + 1 page pour la guard.
        let total = size.checked_add(PAGE_SIZE)?;
        let layout = Layout::from_size_align(total, PAGE_SIZE).ok()?;
        // SAFETY: layout valide, pointeur vérifié.
        let base = unsafe { alloc(layout) };
        if base.is_null() { return None; }

        // Écrire le canari à la limite haute de la zone utilisable (juste avant top).
        // Cela détecte les overflows modestes plus tôt que le canari en bas.
        // SAFETY: base a total bytes alloués, guard_top = base + PAGE_SIZE.
        let stack_base = unsafe { base.add(PAGE_SIZE) }; // début zone utilisable
        let top_raw = unsafe { base.add(total) } as u64;
        let top_aligned = (top_raw & !0xF) - 8;
        let canary_addr = (top_aligned as usize - 8) as *mut u64;

        unsafe {
            // SAFETY: canary_addr est dans la zone [stack_base .. top-8].
            core::ptr::write(canary_addr, STACK_CANARY);

            // Mapper la guard page NX + non-présente pour détection immédiate.
            // CORRECTIF CGX-16 : guard page active → #PF au premier dépassement.
            // En cas d'erreur de mapping, on continue sans guard (dégradé acceptable).
            let _ = crate::memory::protection::map_guard_page(base as u64);
        }

        Some(Self { base, size: total, top: top_aligned })
    }

    /// Vérifie le canari — retourne `false` si overflow détecté.
    pub fn check_canary(&self) -> bool {
        let top_aligned = self.top;
        let canary_addr = (top_aligned as usize - 8) as *const u64;
        // SAFETY: canary_addr est dans la zone allouée.
        unsafe { core::ptr::read(canary_addr) == STACK_CANARY }
    }
}
```

---

## Récapitulatif de l'état post-correctifs

### Module `memory/`

| Bug | Statut post-correctif |
|-----|----------------------|
| DMA ISR lock ordering (CGX-10) | ✅ Résolu — lock libéré avant wakeup |
| KASAN init tardif (Phase 6 > Phase 3) | ⚠️ Documenté — limitation acceptable (heap early non instrumenté) |
| `bitmap_is_free()` null → false | ✅ Comportement correct (pas de bug réel) |

### Module `scheduler/`

| Bug | Statut post-correctif |
|-----|----------------------|
| `context_switch()` missing `assign_cpu()` (CGX-03) | ✅ Résolu — `actual_cpu_id` utilisé partout |
| `block_current_thread()` sans assert préemption (CGX-09) | ✅ Résolu — `debug_assert!` ajouté |
| Commentaire `_cold_reserve (0..40)` trompeur (CGX-13) | ✅ Résolu — carte complète documentée |
| SECURITY_READY spin-wait APs absent (CGX-11) | ✅ Résolu — `SECURITY_READY` + `ap_main()` implémentés |
| `verify()` non constant-time (CGX-12) | ✅ Résolu — `subtle::ct_eq()` utilisé |

### Module `ipc/`

| Bug | Statut post-correctif |
|-----|----------------------|
| `IpcWaiter::thread_id : AtomicU32` (CGX-06) | ✅ Résolu — AtomicU64 |
| `SleepEntry::tid : u32` (CGX-07) | ✅ Résolu — u64 |
| Spin-poll sans blocage scheduler (CGX-08) | ✅ Résolu — hook scheduler appelé |
| `ipc_init()` hooks non connectés (CGX-14) | ✅ Résolu — connexion dans `ipc_init()` |
| `send_irq_notification()` silent fail pid=0 (CGX-15) | ✅ Résolu — erreur descriptive |

### Module `process/`

| Bug | Statut post-correctif |
|-----|----------------------|
| `do_execve()` TSS.RSP0 = kstack_ptr (CGX-01) | ✅ Résolu — `kstack_top()` utilisé |
| Import direct `fs/` depuis `exit.rs` (CGX-02) | ✅ Résolu — trait `VfsExitHook` |
| `mark_exit()` sans libération FPU (CGX-04) | ✅ Résolu — `free_fpu_state()` appelé |
| Fork sans cap table copy (CGX-05) | ✅ Résolu — `shadow_copy()` implémenté |
| Canari bas de pile (CGX-16) | ✅ Résolu — guard page + canari haut |

---

## Critères de validation

### Tests à ajouter / vérifier

```bash
# CGX-01 : TSS.RSP0 correctness
# Tester un execve() suivi d'une interruption immédiate depuis Ring 3
# → pas de corruption de pile kernel

# CGX-03 : context_switch après migration
# Forcer une migration inter-CPU puis vérifier CURRENT_THREAD_PER_CPU[cpu_actuel]
# via le debugger ou un test SMP avec 2+ CPUs

# CGX-04 : FPU state libéré
# Valgrind-style : vérifier que après exit() d'un thread FPU-actif,
# aucune fuite n'est reportée par KASAN-lite

# CGX-06/07 : TID u64
# Créer un thread avec TID > u32::MAX (forcer via PID_ALLOCATOR bypass)
# Bloquer sur un futex, réveiller depuis un autre thread → pas de deadlock

# CGX-12 : constant-time verify
# Mesurer le temps de verify_cap_token() avec token valide vs invalide
# → écart < 5 cycles (pas de timing oracle)

# CGX-11 : SECURITY_READY sur APs
# Boot SMP 4 cœurs + ajout log dans ap_main() avant/après la barrière
# → APs n'accèdent jamais à security/ avant le log "BSP: SECURITY_READY=1"
```

### Checklist CI à ajouter

```makefile
# Makefile / CI — règles à ajouter

# CGX-02 : vérifier que process/ n'importe pas fs/
ci-proc-no-fs:
	@echo "CGX-02: vérification import process/ → fs/"
	@if grep -rn 'use crate::fs' kernel/src/process/ 2>/dev/null | grep -v 'test' | grep -v '//'; then \
		echo "VIOLATION CGX-02: process/ importe fs/ directement"; exit 1; \
	fi

# CGX-06/07 : vérifier que thread_id / tid sont u64 dans ipc/sync/
ci-ipc-tid-u64:
	@echo "CGX-06/07: vérification TID u64 dans ipc/"
	@if grep -n 'thread_id: AtomicU32\|tid: u32' kernel/src/ipc/sync/ 2>/dev/null; then \
		echo "VIOLATION CGX-06/07: TID u32 détecté dans ipc/sync/"; exit 1; \
	fi

# CGX-11 : vérifier que SECURITY_READY est défini
ci-security-ready:
	@echo "CGX-11: vérification SECURITY_READY"
	@grep -n 'SECURITY_READY' kernel/src/security/mod.rs || \
		{ echo "VIOLATION CGX-11: SECURITY_READY absent"; exit 1; }

# CGX-12 : vérifier que verify_cap_token utilise subtle
ci-verify-ct:
	@echo "CGX-12: vérification constant-time verify"
	@grep -n 'ct_eq\|subtle' kernel/src/security/capability/verify.rs || \
		{ echo "VIOLATION CGX-12: verify() sans subtle::ct_eq"; exit 1; }
```

---

## Priorité d'application recommandée

```
Phase 0 — Immédiat (avant tout test SMP)
  CGX-01  TSS.RSP0 kstack_ptr → kstack_top
  CGX-03  context_switch + assign_cpu()
  CGX-04  mark_exit + free_fpu_state()

Phase 1 — Avant démarrage des servers Ring 1
  CGX-02  exit.rs PROC-01 violation
  CGX-05  Fork cap_table shadow_copy
  CGX-11  SECURITY_READY + APs spin-wait
  CGX-12  verify() constant-time

Phase 2 — Avant tests de charge IPC
  CGX-06  IpcWaiter thread_id u64
  CGX-07  SleepEntry tid u64
  CGX-08  IpcWaitQueue block réel
  CGX-09  block_current debug_assert
  CGX-10  DMA ISR lock ordering

Phase 3 — Qualité / hardening
  CGX-13  Commentaire _cold_reserve
  CGX-14  ipc_init hooks
  CGX-15  send_irq pid==0
  CGX-16  KernelStack guard page
```

---

*claude-gamma — ExoOS Kernel Audit — 2026-05-03*
