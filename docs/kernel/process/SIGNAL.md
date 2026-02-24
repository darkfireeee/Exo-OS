# `process/signal/` — Signaux POSIX

> Sources : `kernel/src/process/signal/{default,queue,mask,delivery,handler}.rs`

---

## Table des matières

1. [default.rs — Définitions POSIX et actions par défaut](#1-defaultrs--définitions-posix-et-actions-par-défaut)
2. [queue.rs — Files de signaux](#2-queuers--files-de-signaux)
3. [mask.rs — Masque de signaux](#3-maskrs--masque-de-signaux)
4. [delivery.rs — Livraison et gestion](#4-deliveryrs--livraison-et-gestion)
5. [handler.rs — Table des handlers sigaction](#5-handlerrs--table-des-handlers-sigaction)

---

## 1. `default.rs` — Définitions POSIX et actions par défaut

### `Signal` — Numéros POSIX

```rust
#[repr(u8)]
pub enum Signal {
    SIGHUP    =  1,  SIGINT    =  2,  SIGQUIT   =  3,  SIGILL    =  4,
    SIGTRAP   =  5,  SIGABRT   =  6,  SIGBUS    =  7,  SIGFPE    =  8,
    SIGKILL   =  9,  SIGUSR1   = 10,  SIGSEGV   = 11,  SIGUSR2   = 12,
    SIGPIPE   = 13,  SIGALRM   = 14,  SIGTERM   = 15,  SIGSTKFLT = 16,
    SIGCHLD   = 17,  SIGCONT   = 18,  SIGSTOP   = 19,  SIGTSTP   = 20,
    SIGTTIN   = 21,  SIGTTOU   = 22,  SIGURG    = 23,  SIGXCPU   = 24,
    SIGXFSZ   = 25,  SIGVTALRM = 26,  SIGPROF   = 27,  SIGWINCH  = 28,
    SIGIO     = 29,  SIGPWR    = 30,  SIGSYS    = 31,
}

impl Signal {
    pub const NSIG:     usize = 64;
    pub const SIGRTMIN: u8    = 32;   // Premier RT signal
    pub const SIGRTMAX: u8    = 63;   // Dernier RT signal
}
```

Méthodes utilitaires :

| Méthode | Description |
|---------|-------------|
| `from_u8(n) -> Option<Self>` | Conversion sécurisée depuis un entier |
| `number(self) -> u8` | Numéro du signal |
| `is_realtime(n: u8) -> bool` | Vrai si RT signal (32..63) |
| `is_blockable(self) -> bool` | Faux pour `SIGKILL` et `SIGSTOP` |
| `is_ignorable(self) -> bool` | Faux pour `SIGKILL` et `SIGSTOP` |

### `SigActionKind` et `SigAction`

```rust
pub enum SigActionKind {
    Default,                    // Action par défaut
    Ignore,                     // Ignorer le signal
    Handler(u64),               // Adresse userspace du handler
    SigAction(u64),             // Adresse sigaction(3) (avec siginfo_t)
}

pub struct SigAction {
    pub kind:    SigActionKind,
    pub mask:    SigMask,       // Signaux supplémentaires à bloquer pendant handler
    pub flags:   u32,           // SA_RESTART, SA_NODEFER, SA_SIGINFO, etc.
}
```

### `default_action(sig) -> DefaultAction`

Retourne l'action POSIX par défaut pour chaque signal :

| Action | Signaux |
|--------|---------|
| `Terminate` | `SIGHUP`, `SIGINT`, `SIGPIPE`, `SIGTERM`, `SIGALRM`, … |
| `CoreDump` | `SIGQUIT`, `SIGILL`, `SIGABRT`, `SIGFPE`, `SIGSEGV`, `SIGBUS`, … |
| `Ignore` | `SIGCHLD`, `SIGURG`, `SIGWINCH`, `SIGCONT` |
| `Stop` | `SIGSTOP`, `SIGTSTP`, `SIGTTIN`, `SIGTTOU` |
| `Continue` | `SIGCONT` (si process stoppé) |

---

## 2. `queue.rs` — Files de signaux

### `SigInfo` — Informations associées (`siginfo_t`)

```rust
#[repr(C)]
pub struct SigInfo {
    pub signo:      u32,  // Numéro du signal
    pub code:       i32,  // SI_USER(0), SI_KERNEL(0x80), SI_QUEUE(-1), ...
    pub sender_pid: u32,
    pub sender_uid: u32,
    pub value_int:  i32,  // POSIX.1b sigqueue() valeur entière
    pub value_ptr:  u64,  // POSIX.1b sigqueue() valeur pointeur
    pub fault_addr: u64,  // Adresse fautive (SIGSEGV, SIGFPE)
}
```

Constructeurs : `SigInfo::from_kill(sig, pid, uid)`, `SigInfo::kernel(sig)`.

### `SigQueue` — Signaux standard (1..31)

```rust
pub struct SigQueue {
    pub pending: AtomicU64,  // Un bit par signal, bit i = signal (i+1) en attente
}
```

Un seul bit par signal : plusieurs `send` du même signal = **1 seule livraison**.

| Méthode | Description |
|---------|-------------|
| `enqueue(sig: u8)` | `fetch_or(1 << (sig-1), AcqRel)` |
| `dequeue() -> u8` | `trailing_zeros()` du bitmap + `fetch_and(!mask)` |
| `is_pending(sig: u8) -> bool` | Test du bit |
| `is_empty() -> bool` | `pending == 0` |

### `RTSigQueue` — RT Signaux (32..63)

```rust
pub const SIGQUEUE_DEPTH: usize = 32;  // Max 32 occurrences par signal
const RT_NSIG: usize = 32;             // Signaux 32..63

pub struct RTSigQueue {
    // Tableau fixe par signal, sans alloc dynamique
    entries: UnsafeCell<[[SigInfo; SIGQUEUE_DEPTH]; RT_NSIG]>,
    heads:   [AtomicU32; RT_NSIG],    // indices de tête (consommation)
    tails:   [AtomicU32; RT_NSIG],    // indices de queue (production)
    counts:  [AtomicU32; RT_NSIG],    // nombre en attente par signal
}

pub static SIGQUEUE_OVERFLOW: AtomicU64;  // Compteur de dépassements
```

**POSIX.1b** : au moins une occurrence par RT signal est garantie.  
En cas de dépassement de `SIGQUEUE_DEPTH` : le signal est perdu + `SIGQUEUE_OVERFLOW++`.

| Méthode | Description |
|---------|-------------|
| `enqueue(sig: u8, info: SigInfo) -> bool` | Empile ; retourne `false` si plein |
| `dequeue(sig: u8) -> Option<SigInfo>` | Dépile (FIFO) |
| `is_any_pending() -> bool` | Vrai si au moins un RT signal en attente |

---

## 3. `mask.rs` — Masque de signaux

### `SigMask` — Valeur pure (non atomique)

```rust
#[repr(transparent)]
pub struct SigMask(pub u64);
// Bit i = signal (i+1) est BLOQUÉ.
// SIGKILL (bit 8) et SIGSTOP (bit 18) sont toujours forcés à 0.

const NON_BLOCKABLE: u64 =
    (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
```

| Méthode | Description |
|---------|-------------|
| `set(sig)` | Bloque le signal (ignore les non-bloquables) |
| `clear(sig)` | Débloque le signal |
| `is_set(sig) -> bool` | Test |
| `union(other) -> SigMask` | OU bitwise (en respectant NON_BLOCKABLE) |
| `intersect(other) -> SigMask` | ET bitwise |
| `difference(other) -> SigMask` | `self & !other` |

Constantes : `SigMask::EMPTY`, `SigMask::FULL` (tous sauf non-bloquables).

### `SigSet` — Wrapper atomique (stockage dans TCB)

```rust
#[repr(transparent)]
pub struct SigSet(AtomicU64);
```

Permet d'une part la lecture lockless du masque depuis le scheduler (`load(Acquire)`), d'autre part la modification atomique.

### API `sigprocmask`

```rust
pub fn sigprocmask(
    how:        i32,          // SIG_BLOCK | SIG_UNBLOCK | SIG_SETMASK
    set:        SigMask,      // Nouvel ensemble
    tcb:        &ThreadControlBlock,
) -> Result<SigMask, SigmaskError>;
```

### `reset_signals_on_exec(thread)`

Réinitialise le masque de signaux à `EMPTY` et tous les handlers installés à `SIG_DFL`.  
Appelé par `exec.rs` après le chargement ELF.

---

## 4. `delivery.rs` — Livraison et gestion

### Règle SIGNAL-01

> `handle_pending_signals()` est appelé **uniquement au retour de syscall** (entrée assembleur `syscall_entry`), jamais depuis une interruption ou un context switch.

### `send_signal_to_pid(pid, sig) -> Result<(), SendError>`

```
  1. Trouver le PCB via PROCESS_REGISTRY (lockless).
  2. Vérifier que l'état est != Zombie et != Dead.
  3. Obtenir le pointeur du thread principal (pcb.main_thread_ptr()).
  4. Mettre le signal en file :
       → sig < 32 : thread.sig_queue.enqueue(sig)
       → sig ≥ 32 : thread.rt_sig_queue.enqueue(sig, SigInfo::kernel(sig))
  5. thread.raise_signal_pending()  ← positionne TCB.signal_pending=true (PROC-04)
```

### `send_signal_to_tcb(thread, sig, info)`

Variante pour la livraison directe (exceptions matérielles : `#GP`, `#PF`, `#DE`).  
Fournit un `SigInfo` complet avec l'adresse fautive.

### `SyscallContext` — Registres au retour syscall

```rust
pub struct SyscallContext {
    pub rip:    *mut u64,  // instruction pointer userspace
    pub rsp:    *mut u64,  // stack pointer userspace
    pub rax:    *mut u64,  // valeur de retour syscall
    pub rdi:    *mut u64,  // arg1 (handler_fn pour signal)
    pub rsi:    *mut u64,  // arg2 (siginfo_t*)
    pub rdx:    *mut u64,  // arg3 (ucontext_t*)
    pub saved_mask: SigMask,
}
```

### `handle_pending_signals(ctx, thread, pcb)`

```
  Boucle sur les signaux pending :
    1. Lire SigQueue.dequeue() puis RTSigQueue.dequeue().
    2. Vérifier sig_mask (signal bloqué ? → laisser en attente).
    3. Chercher l'action dans SigHandlerTable :
       a. SIG_DFL → default_action() :
            Terminate → do_exit(sig_as_exit_code)
            CoreDump  → marquer NO_DUMP + do_exit()
            Stop      → transition Stopped + schedule_block()
            Ignore    → continuer
       b. SIG_IGN → continuer
       c. Handler(fn_ptr) → set up stack userspace :
            push siginfo_t, ucontext_t sur pile user.
            modifier ctx.rip = fn_ptr, ctx.rdi = signo.
            mask |= sig_action.mask (bloquer pendant handler).
  Fin de boucle.
```

### `SendError`

| Variante | Cause |
|----------|-------|
| `NoSuchProcess` | PID inexistant dans la registry |
| `InvalidSignal` | `sig == 0` ou `sig > 63` |
| `PermissionDenied` | Credentials insuffisants (envoi entre processus) |

---

## 5. `handler.rs` — Table des handlers sigaction

### `SigHandlerTable`

```rust
pub struct SigHandlerTable {
    handlers: [SigAction; Signal::NSIG],  // Index = numéro signal
}
```

Protégée par `pcb.sig_handlers: SpinLock<SigHandlerTable>`.

### API

| Méthode | Signature | Description |
|---------|-----------|-------------|
| `get_action` | `(sig: u8) -> SigAction` | Lit l'action courante |
| `set_action` | `(sig: u8, action: SigAction) -> Result<(), SigactionError>` | Installe un handler |
| `reset_all_to_default` | `()` | Remet tous les handlers à `SIG_DFL` (pour exec) |

### Vérifications `set_action()`

- `SIGKILL` et `SIGSTOP` : `set_action()` retourne `SigactionError::NotPermitted`.
- RT signals (32..63) : acceptés avec `SigActionKind::Handler` ou `SigAction`.
- `SA_SIGINFO` flag → la fonction reçoit `(int sig, siginfo_t*, ucontext_t*)`.

### Flags SA_* supportés

| Flag | Valeur | Effet |
|------|--------|-------|
| `SA_RESTART` | bit 0 | Redémarre automatiquement les syscalls interrompus |
| `SA_NODEFER` | bit 1 | Ne pas bloquer le signal pendant son handler |
| `SA_SIGINFO` | bit 2 | Handler reçoit `siginfo_t` + `ucontext_t` |
| `SA_RESETHAND` | bit 3 | Remet l'action à `SIG_DFL` après le premier appel |
| `SA_ONSTACK` | bit 4 | Utilise la `sigaltstack` si disponible |
