# `process/thread/` — Threads POSIX

> Sources : `kernel/src/process/thread/{creation,join,detach,local_storage,pthread_compat}.rs`

---

## Table des matières

1. [creation.rs — Création d'un thread POSIX](#1-creationrs--création-dun-thread-posix)
2. [join.rs — Attente de terminaison](#2-joinrs--attente-de-terminaison)
3. [detach.rs — Détachement](#3-detachrs--détachement)
4. [local_storage.rs — TLS (Thread Local Storage)](#4-local_storagrs--tls-thread-local-storage)
5. [pthread_compat.rs — Couche de compatibilité syscall](#5-pthread_compatrs--couche-de-compatibilité-syscall)

---

## 1. `creation.rs` — Création d'un thread POSIX

**Objectif de performance** : création en **< 500 ns** (PROC-09) mesurée depuis `clone()`.

### `ThreadAttr` — Attributs du thread

```rust
pub struct ThreadAttr {
    pub stack_size:       u64,     // Taille pile user (défaut : 8 MiB)
    pub stack_addr:       u64,     // Base pile fournie (0 = allouer)
    pub policy:           SchedPolicy,
    pub priority:         Priority,
    pub detached:         bool,    // Détaché dès la création
    pub cpu_affinity:     i32,     // -1 = sans préférence
    pub sigaltstack_size: u64,     // Taille sigaltstack (défaut : 8192)
}
```

### `ThreadCreateParams`

```rust
pub struct ThreadCreateParams {
    pub pcb:         *const ProcessControlBlock,
    pub attr:        ThreadAttr,
    pub start_func:  u64,   // Adresse entry userspace
    pub arg:         u64,   // Argument (registre RDI)
    pub target_cpu:  u32,
    pub pthread_out: u64,   // Adresse struct pthread_t userspace à remplir
}
```

### Séquence `create_thread(params)`

```
  1. Vérifier que le processus n'est pas EXITING.
  2. TID_ALLOCATOR.alloc() → TID.
  3. ProcessThread::new(pid, tid, policy, priority, addr) → Box<ProcessThread>.
  4. Configurer ThreadAddress (entry_point, initial_rsp, tls_base).
  5. pcb.inc_threads().
  6. Écrire pthread_t dans userspace (si pthread_out != 0).
  7. PreemptGuard::new() + run_queue(cpu).enqueue(tcb_ptr).
  8. Retourner ThreadHandle { tid, thread: raw_ptr }.
```

### `ThreadHandle`

```rust
pub struct ThreadHandle {
    pub tid:    Tid,
    pub thread: *mut ProcessThread,   // raw ptr, transféré à l'appelant
}
```

### `ThreadCreateError`

| Variante | Cause |
|----------|-------|
| `TidExhausted` | Plus de TIDs |
| `OutOfMemory` | Échec d'allocation stack ou ProcessThread |
| `InvalidCpu` | CPU hors bornes |
| `ProcessExiting` | Flag `EXITING` positionné dans le PCB |
| `StackSetupFailed` | Échec d'initialisation de la pile utilisateur |

---

## 2. `join.rs` — Attente de terminaison

### Mécanisme

Le join repose sur **trois atomiques** dans `ProcessThread` :

| Champ | Type | Rôle |
|-------|------|------|
| `detached` | `AtomicBool` | Le thread est-il détaché ? |
| `join_done` | `AtomicBool` | Le thread a-t-il terminé ? |
| `join_result` | `AtomicU64` | Valeur de retour (valeur de `pthread_exit()`) |

Et une **`WaitQueue` globale** `JOIN_WAIT` pour les threads bloqués en attente.

### `thread_join(target, caller_tcb) -> Result<u64, JoinError>`

```
  Boucle spurious-wakeup-safe :
    1. Si target.detached → Err(Detached).
    2. Si target.join_done (Acquire) → Ok(join_result).
    3. Si caller_tcb.has_signal_pending() → Err(Interrupted).
    4. unsafe { JOIN_WAIT.wait_interruptible(caller_tcb as *mut _) }.
    5. Retour à 1.
```

### `wake_joiners()`

Appelé par `do_exit_thread()` juste avant `schedule_block()`.  
Réveille **tous** les threads bloqués sur `JOIN_WAIT` (ils re-testent `join_done`).

```rust
pub fn wake_joiners() {
    JOIN_WAIT.notify_all();
}
```

### `JoinError`

| Variante | Cause |
|----------|-------|
| `Detached` | Thread créé avec `attr.detached=true` ou `detach()` appelé |
| `AlreadyJoined` | Déjà joint par un autre thread |
| `Interrupted` | Signal reçu pendant l'attente (`EINTR`) |
| `InvalidThread` | Pointeur `target` null ou invalide |

---

## 3. `detach.rs` — Détachement

```rust
/// Détache le thread cible : ses ressources seront libérées automatiquement
/// à sa terminaison sans nécessiter de pthread_join().
pub fn thread_detach(target: *mut ProcessThread) -> Result<(), DetachError>;
```

Opération : `target.detached.store(true, Release)`.  
Erreur si le thread est déjà detaché (`AlreadyDetached`) ou déjà terminé (`AlreadyDone`).

---

## 4. `local_storage.rs` — TLS (Thread Local Storage)

### Architecture x86_64

```
MSR_GS_BASE (0xC000_0101)  ← GS.base userspace = adresse du bloc TLS
MSR_KERNEL_GS_BASE         ← GS.base kernel (CPU state)
SWAPGS instruction         ← bascule entre les deux
```

Au retour en userspace (`sysret` / `iret`) : `SWAPGS` → GS pointe sur le bloc TLS du thread.

### `TlsBlock` — Bloc TLS statique

```rust
pub struct TlsBlock {
    data:       Box<[u8]>,   // tdata (copie) + tbss (zéros)
    tdata_size: usize,
    total_size: usize,
    user_base:  u64,         // adresse userspace de ce bloc
}

pub const TLS_MAX_SIZE: usize = 65_536;  // 64 KiB maximum
```

Méthodes :

| Méthode | Description |
|---------|-------------|
| `new(tdata, tbss_size, user_base)` | Crée en copiant le modèle `tdata` ; remplit `tbss` à zéro |
| `clone_for_thread(new_user_base)` | Clone lors d'un `pthread_create()` |
| `as_ptr() -> *const u8` | Pointeur vers les données (pour écriture dans `MSR_GS_BASE`) |
| `user_base() -> u64` | Adresse userspace |

### `TlsKey` — Clés dynamiques (pthread_key_t)

```rust
pub struct TlsKey(pub u32);     // TlsKey::INVALID = u32::MAX

pub const MAX_TLS_KEYS: usize = 1024;
```

### `TlsRegistry` — Registre global des clés

```rust
pub struct TlsKey {
    pub keys: UnsafeCell<[TlsKeyEntry; MAX_TLS_KEYS]>,
    alloc_map: AtomicU64,                 // bitmap d'allocation (premiers 64)
    count:     AtomicU32,
    lock:      SpinLock<()>,
}

pub struct TlsKeyEntry {
    pub in_use:      AtomicU32,
    pub destructor:  AtomicUsize,        // fn(*mut ()) optionnelle
}
```

API :

| Fonction | Description |
|----------|-------------|
| `tls_key_create(destructor) -> Option<TlsKey>` | Alloue une clé (avec destructeur optionnel) |
| `tls_key_delete(key)` | Libère la clé |
| `tls_set_value(key, value, thread)` | Stocke une valeur par thread |
| `tls_get_value(key, thread) -> u64` | Lit la valeur |

### Initialisation au thread start

```
execve() → fs/ fournit (tdata_ptr, tdata_size, tbss_size) dans ElfLoadResult
  ↓
TlsBlock::new(tdata, tbss_size, user_base) → TlsBlock
  ↓
thread.tls_base_atomic.store(tls_block.as_ptr() as u64)
  ↓
wrmsrl(MSR_GS_BASE, tls_block.as_ptr() as u64)   ← arch/x86_64
```

---

## 5. `pthread_compat.rs` — Couche de compatibilité syscall

Fournit les entrées syscall POSIX pour la libc :

| Syscall | Fonction noyau | Description |
|---------|----------------|-------------|
| `clone(flags, stack, ptid, tls, ctid)` | `sys_clone()` | Crée un thread via `create_thread()` |
| `pthread_exit(retval)` | `sys_pthread_exit()` | Appelle `do_exit_thread()` |
| `pthread_join(tid, retval_ptr)` | `sys_pthread_join()` | Appelle `thread_join()` |
| `pthread_detach(tid)` | `sys_pthread_detach()` | Appelle `thread_detach()` |
| `set_tid_address(tidptr)` | `sys_set_tid_address()` | Enregistre le TID pointer (cleartid) |
| `arch_prctl(ARCH_SET_GS, base)` | `sys_arch_prctl_gs()` | Écrit `MSR_GS_BASE` |

### Traduction d'erreurs → errno

```
ThreadCreateError::TidExhausted  → EAGAIN
ThreadCreateError::OutOfMemory   → ENOMEM
ThreadCreateError::ProcessExiting → ESRCH
JoinError::Detached              → EINVAL
JoinError::Interrupted           → EINTR
JoinError::AlreadyJoined         → EINVAL
```

### Interaction avec les signaux

Tous les appels bloquants (`pthread_join`, `pthread_exit` si attente de join) vérifient `caller_tcb.has_signal_pending()` pour satisfaire `EINTR` (conforme POSIX).
