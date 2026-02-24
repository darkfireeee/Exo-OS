# Module `process/` — Documentation complète

> **Exo-OS** · Couche 1.5 · `kernel/src/process/`  
> Dernière mise à jour : session de documentation complète

---

## Table des matières

| Fichier | Contenu |
|---------|---------|
| [CORE.md](CORE.md) | `pid`, `tcb`, `pcb`, `registry` — structures fondamentales |
| [LIFECYCLE.md](LIFECYCLE.md) | `create`, `fork`, `exec`, `exit`, `wait`, `reap` — cycle de vie |
| [THREAD.md](THREAD.md) | `creation`, `join`, `detach`, `local_storage`, `pthread_compat` — threads POSIX |
| [SIGNAL.md](SIGNAL.md) | `delivery`, `handler`, `mask`, `queue`, `default` — signaux POSIX |
| [STATE_GROUP_NS.md](STATE_GROUP_NS.md) | `state`, `group`, `namespace`, `resource` — états, sessions, namespaces, limites |

---

## 1. Rôle et position architecturale

```
┌──────────────────────────────────────────────────────────┐
│   Couche 3 : fs/  (VFS, ELF)  ─── trait ElfLoader ──►   │
│   Couche 2 : ipc/ (IPC, sockets) ─── pas d'import ──►   │
├──────────────────────────────────────────────────────────┤
│   Couche 1.5 : process/  (ce module)                     │
│   • gestion PIDs / TIDs / PCB / TCB                      │
│   • cycle de vie : create, fork, exec, exit, wait        │
│   • signaux POSIX (delivery, masque, handler)            │
│   • threads POSIX (création, join, TLS)                  │
│   • namespaces, sessions, rlimits                        │
├──────────────────────────────────────────────────────────┤
│   Couche 1   : scheduler/  (TCB, run queue, wait_queue)  │
│   Couche 0   : memory/     (allocateur, CoW, DMA)        │
└──────────────────────────────────────────────────────────┘
```

**Règle PROC-01** : `process/` **ne doit jamais importer** `fs/` ni `ipc/` directement.  
La communication avec `fs/` passe par des **traits injectés au boot** (`ElfLoader`, `AddressSpaceCloner`).

---

## 2. Règles de la Refonte (PROC-01 → PROC-10)

| ID | Règle | Implémentation |
|----|-------|----------------|
| PROC-01 | Pas d'import `fs/` / `ipc/` direct | Traits `ElfLoader`, `AddressSpaceCloner` |
| PROC-02 | `DmaWakeupHandler` implémenté par `process/` | `state/wakeup.rs::ProcessWakeupHandler` |
| PROC-03 | `PID_MAX ≤ 32 767` | `PID_BITMAP_WORDS=512` → 32 768 slots |
| PROC-04 | `signal_pending` visible du scheduler | `ThreadControlBlock::signal_pending: AtomicBool` |
| PROC-05 | Transitions d'état PCB validées | `state/transitions.rs::transition()` |
| PROC-06 | `fork()` flush TLB parent avant retour | `AddressSpaceCloner::flush_tlb_after_fork()` |
| PROC-07 | Reaper = kthread dédié, jamais inline | `lifecycle/reap.rs::REAPER_QUEUE::kthread_reaper()` |
| PROC-08 | TLB flush dans fork | identique à PROC-06 |
| PROC-09 | Création thread `<500 ns` | structures lock-free, enqueue direct |
| PROC-10 | Signaux livrés au retour syscall uniquement | `signal/delivery.rs::SIGNAL-01` |

---

## 3. Contrat UNSAFE (regle_bonus.md)

Tout bloc `unsafe {}` est précédé d'un commentaire `// SAFETY: ...` expliquant :
- la validité des pointeurs déréférencés,
- les invariants qui garantissent l'absence de data-race,
- pourquoi l'opération est correcte dans ce contexte.

Vérifié par audit automatique (script PowerShell) → **0 violation** au dernier `cargo check`.

---

## 4. Arborescence complète

```
process/
├── mod.rs                    ← pub use de tous les sous-modules, fn init()
├── core/
│   ├── mod.rs
│   ├── pid.rs                ← Pid, Tid, PidAllocator (bitmap CAS)
│   ├── tcb.rs                ← ProcessThread, KernelStack, ThreadAddress
│   ├── pcb.rs                ← ProcessControlBlock, ProcessState, OpenFileTable
│   └── registry.rs           ← ProcessRegistry (lockless reads, spinlock writes)
├── lifecycle/
│   ├── mod.rs
│   ├── create.rs             ← create_process(), create_kthread()
│   ├── fork.rs               ← fork() CoW + ForkFlags
│   ├── exec.rs               ← execve() via trait ElfLoader
│   ├── exit.rs               ← do_exit(), do_exit_thread()
│   ├── wait.rs               ← waitpid(), WaitOptions, WaitTable
│   └── reap.rs               ← kthread reaper, REAPER_QUEUE
├── thread/
│   ├── mod.rs
│   ├── creation.rs           ← create_thread() <500ns
│   ├── join.rs               ← thread_join(), wake_joiners()
│   ├── detach.rs             ← thread_detach()
│   ├── local_storage.rs      ← TlsBlock, TlsRegistry, GS.base
│   └── pthread_compat.rs     ← syscalls pthread_create/join/exit/key
├── signal/
│   ├── mod.rs
│   ├── default.rs            ← Signal enum POSIX, SigAction, default_action()
│   ├── queue.rs              ← SigQueue (bitmap), RTSigQueue (tableau fixe), SigInfo
│   ├── mask.rs               ← SigMask, SigSet (atomique), sigprocmask
│   ├── delivery.rs           ← send_signal_to_pid(), handle_pending_signals()
│   └── handler.rs            ← SigHandlerTable, sigaction(2)
├── state/
│   ├── mod.rs
│   ├── transitions.rs        ← machine à états PCB, transition()
│   └── wakeup.rs             ← ProcessWakeupHandler implémente DmaWakeupHandler
├── group/
│   ├── mod.rs
│   ├── session.rs            ← Session, SessionTable, setsid()
│   ├── pgrp.rs               ← ProcessGroup, GroupTable, setpgid()
│   └── job_control.rs        ← contrôle de job POSIX (SIGTSTP/SIGCONT/SIGTTIN/SIGTTOU)
├── namespace/
│   ├── mod.rs
│   ├── pid_ns.rs             ← PidNamespace, NsPidBitmap, ROOT_PID_NS
│   ├── mount_ns.rs           ← MountNamespace (stub léger)
│   ├── net_ns.rs             ← NetNamespace (stub léger)
│   ├── uts_ns.rs             ← UtsNamespace (hostname, domainname)
│   └── user_ns.rs            ← UserNamespace, uid/gid mapping
└── resource/
    ├── mod.rs
    ├── rlimit.rs             ← RLimitKind, RLimit, RLimitTable, getrlimit/setrlimit
    ├── usage.rs              ← ProcessUsage (utime, stime, faults, io)
    └── cgroup.rs             ← CgroupEntry, CgroupTable (gestion des cgroups v1)
```

---

## 5. Séquence d'initialisation

```rust
// kernel/src/process/mod.rs::init()
pub fn init() {
    // 1. Initialiser les allocateurs PID/TID.
    unsafe { pid::init(PID_MAX, TID_MAX); }           // réserve 0 et 1
    // 2. Créer le processus idle (PID 0, kernel thread).
    lifecycle::create::create_kthread(...);
    // 3. Créer init (PID 1).
    lifecycle::create::create_process(CreateParams { ppid: Pid::IDLE, ... });
    // 4. Enregistrer le DMA wakeup handler.
    state::wakeup::register_with_dma();
}
```

---

## 6. Dépendances inter-modules

```
process/core/pid        → aucune dépendance kernel
process/core/pcb        → scheduler::sync::spinlock, process::signal::*
process/core/tcb        → scheduler::core::task (ThreadControlBlock, TaskState)
process/core/registry   → process::core::pcb, scheduler::sync::spinlock
process/lifecycle/*     → process::core::*, scheduler::core::*, memory (traits)
process/signal/*        → process::core::*, scheduler::core::task
process/thread/*        → process::core::*, scheduler::sync::wait_queue
process/state/*         → process::core::pcb, memory::dma (trait)
process/group/*         → process::core::pid, scheduler::sync::spinlock
process/namespace/*     → scheduler::sync::spinlock, core::sync::atomic
process/resource/*      → aucune dépendance kernel directe
```

---

## 7. Performances clés

| Opération | Objectif | Mécanisme |
|-----------|----------|-----------|
| Allocation PID/TID | O(1) amorti | Bitmap + CLZ + CAS |
| Lookup process par PID | O(1) lockless | `AtomicPtr::load(Acquire)` |
| Création thread | < 500 ns | TID alloc + TCB init + enqueue direct |
| Test signal pending (scheduler) | 1 instruction | `AtomicBool::load(Acquire)` dans TCB |
| fork() | O(pages mappées) | CoW délégué à `memory/cow/` |

---

*Voir chaque fichier de documentation pour le détail des APIs, invariants et exemples d'utilisation.*
