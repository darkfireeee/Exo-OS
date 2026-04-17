# Phase 3 — Processus + Signaux POSIX

**Prérequis exo-boot · Modules : `process/`, `syscall/handlers/`, `scheduler/core/`**

> Dépend de Phase 2 (context switch opérationnel, SWAPGS correct, timer calibré).  
> Condition de sortie : fork() fonctionnel, execve() fonctionnel, signaux POSIX livrés.

---

## 1. Vue d'ensemble

Phase 3 couvre le cycle de vie complet des processus et l'infrastructure signal POSIX :

| Module | Rôle | Chemin |
|---|---|---|
| `process/lifecycle/fork.rs` | fork() CoW, PCB/TCB fils, TLB shootdown | `kernel/src/process/lifecycle/` |
| `process/lifecycle/exec.rs` | execve(), chargement ELF via trait | `kernel/src/process/lifecycle/` |
| `process/signal/handler.rs` | Construction frame signal userspace | `kernel/src/process/signal/` |
| `process/signal/delivery.rs` | Livraison signaux pending au retour syscall | `kernel/src/process/signal/` |
| `syscall/dispatch.rs` | Cas spéciaux fork/execve/sigreturn | `kernel/src/syscall/` |
| `syscall/handlers/signal.rs` | sys_rt_sigaction, sys_rt_sigprocmask | `kernel/src/syscall/handlers/` |

---

## 2. Corrections critiques implémentées

### ERR-06 / BUG-04 — IA32_FS_BASE avant jump_to_entry ✅

**Fichier** : `kernel/src/process/lifecycle/exec.rs`

**Problème** : Après `load_elf()`, le MSR `IA32_FS_BASE` (0xC000_0100) n'était pas écrit
avant le saut vers le point d'entrée ELF. Tout accès TLS (thread_local!, malloc, pthread_self)
crashait immédiatement car GS/FS pointait vers l'ancienne base.

**Correction** :
```rust
if elf_result.tls_base != 0 {
    unsafe {
        crate::arch::x86_64::cpu::msr::write_msr(
            crate::arch::x86_64::cpu::msr::MSR_FS_BASE,
            elf_result.tls_base,
        );
    }
}
```

---

### ERR-07 / BUG-05 — is_canonical(rcx) avant SYSRETQ ✅

**Fichier** : `kernel/src/arch/x86_64/syscall.rs`

**Problème** : Si RCX retourné vers userspace n'est pas canonique (bits [63:48] non-nuls ou
non-étendus), SYSRETQ #GP dans le contexte kernel — exploit de ring transition.

**Correction** : Validation canonique de RCX dans le stub ASM syscall_exit avant SYSRETQ.
Voir `arch/x86_64/syscall.rs` `is_canonical()` check.

---

### ERR-11 — block_all_except_kill() durant exec ✅

**Fichier** : `kernel/src/process/lifecycle/exec.rs`

**Problème** : Sans blocage de signaux pendant exec, un signal arrivant entre `load_elf()`
et `reset_signals_on_exec()` pouvait invoquer un handler de l'ancien adress space
partiellement remplacé → RCE garantie.

**Correction** : Appel à `block_all_except_kill(&thread.sched_tcb)` AVANT `load_elf()`.
`reset_signals_on_exec()` débloque les signaux après que l'espace d'adressage est stable.

---

### SIG-01 — SigactionEntry par valeur (non AtomicPtr) ✅

**Fichier** : `kernel/src/process/signal/tcb.rs`

**Problème** : Un AtomicPtr vers SigAction permettait une modification TOCTOU pendant
la livraison du signal (race entre lecture handler et modification par rt_sigaction).

**Correction** : `SigactionEntry` est stockée par valeur dans `SigHandlerTable[64]`.
Lecture atomique protégée par `SpinLock<SigHandlerTable>` dans le PCB.

---

### SIG-07 — SIGKILL / SIGSTOP non-masquables ✅

**Fichier** : `kernel/src/process/signal/mask.rs`

**Problème** : `sigprocmask(SIG_SETMASK, full_mask)` pouvait bloquer SIGKILL et SIGSTOP,
rendant le processus impossible à tuer — déni de service kernel.

**Correction** : Constante `NON_MASKABLE: u64 = (1u64 << 8) | (1u64 << 18)` forçant ces
bits à 0 à chaque écriture du masque (sigprocmask, sigreturn, systrt_sigprocmask).

---

### FORK-02 — CoW + TLB shootdown ✅

**Fichier** : `kernel/src/process/lifecycle/fork.rs`

**Problème** : Sans TLB shootdown IPI vers les autres CPU, les pages CoW fraîchement
marquées read-only étaient encore modifiables via le TLB stale des autres CPU → corruption
mémoire cross-process.

**Correction** : Appel à `cloner.flush_tlb_after_fork(parent_cr3)` depuis `do_fork()`
immédiatement après le clonage CoW (avant enqueue du fils). Le trait `AddressSpaceCloner`
impose cette sémantique sur toute implémentation.

---

### SIG-13 — Magic 0x5349474E vérifié à sigreturn ✅

**Fichier** : `kernel/src/process/signal/handler.rs`

**Problème** : `sys_rt_sigreturn` ne vérifiait pas le magic du frame signal avant de
restaurer l'état. Un attaquant Ring3 pouvait forger un frame avec RIP arbitraire pour
escalader vers des droits supérieurs (SIGSEGV bypass, ROP).

**Architecture** (LAC-01 compliant — constant-time) :
- `setup_signal_frame()` écrit `uc_flags = SIGNAL_FRAME_MAGIC (0x5349_474E)` dans l'UContext
- `restore_signal_frame()` extrait TOUS les registres PUIS vérifie le magic (constant-time)
- Vérification : `magic_diff = uc.uc_flags ^ SIGNAL_FRAME_MAGIC` → fail si non-nul
- Constante de positionnement : `SIGNAL_FRAME_UC_OFFSET = 160` (pretcode+signo+pinfo+puc+SigInfoC)

```rust
// Extraction constant-time : données extraites AVANT le check (LAC-01).
let regs = UContextRegs { rip: uc.uc_mcontext.rip, rsp: uc.uc_mcontext.rsp, ... };
let magic_diff = uc.uc_flags ^ (SIGNAL_FRAME_MAGIC as u64);
if magic_diff != 0 { return None; } // magic invalide
Some(regs)
```

---

## 3. Syscalls implémentés

### sys_rt_sigreturn ✅

**Fichier** : `kernel/src/syscall/dispatch.rs` — cas spécial avant slow-path

Implémenté via `handle_sigreturn_inplace(frame)` qui :
1. Calcule `sig_rsp = frame.rsp - 8` (le `ret` du handler a popped pretcode)
2. Lit `uc_ptr = sig_rsp + SIGNAL_FRAME_UC_OFFSET`
3. Appelle `verify_and_extract_uc(uc_ptr)` pour vérification magic constant-time
4. En cas de succès : restaure rcx (RIP), r11 (RFLAGS), rsp, rax, rdi, rsi, rdx, r8, r9
5. Met à jour `gs:[0x08]` (user_rsp slot du PerCpuData pour SYSRETQ)
6. Restaure le signal_mask dans le TCB (SIGKILL/SIGSTOP non-masquables forcés)
7. En cas d'échec magic : RIP=0 → SIGSEGV Ring3 (jamais de crash Ring0)

---

### sys_rt_sigaction ✅

**Fichier** : `kernel/src/syscall/handlers/signal.rs`

Lecture de l'ancienne action depuis `PCB.sig_handlers` (SpinLock),
installation de la nouvelle action depuis l'ABI Linux `struct sigaction` (sa_handler, sa_flags,
sa_restorer, sa_mask).

---

### sys_rt_sigprocmask ✅

**Fichier** : `kernel/src/syscall/handlers/signal.rs`

Support de `SIG_BLOCK`, `SIG_UNBLOCK`, `SIG_SETMASK`.
Masque lu et écrit via `ThreadControlBlock.signal_mask (AtomicU64)` accessible via `gs:[0x20]`.
SIGKILL (bit 8) et SIGSTOP (bit 18) toujours non-masquables (SIG-07).

---

### sys_fork ✅

**Fichier** : `kernel/src/syscall/dispatch.rs` — cas spécial avant slow-path

Implémenté via `handle_fork_inplace(frame)` qui :
1. Lit TCB courant via `gs:[0x20]` → PID → PCB via `PROCESS_REGISTRY`
2. Récupère `ProcessThread` via `pcb.main_thread_ptr()`
3. Construit `ForkContext { child_rip: frame.rcx, child_rsp: frame.rsp, ... }`
4. Appelle `do_fork(&ctx)` → retourne `child_pid` au parent
5. Le fils est enqueué dans la run queue, démarrera via `fork_child_trampoline`

**Kernel stack fils** (comportement `switch_to_new_thread`) :
```
kernel_rsp + 0..48  : 6 callee-saved regs = 0
kernel_rsp + 48     : fork_child_trampoline (ret address)
kernel_rsp + 56..96 : iretq frame (RIP=child_rip, CS, RFLAGS, RSP=child_rsp, SS)
```

`fork_child_trampoline` (switch_asm.s) : `xor %eax, %eax ; swapgs ; iretq`

---

### sys_execve ✅

**Fichier** : `kernel/src/syscall/dispatch.rs` — cas spécial avant slow-path

Implémenté via `handle_execve_inplace(frame)` qui :
1. Lit path depuis userspace via `read_user_path(frame.rdi)`
2. Récupère TCB → PCB → ProcessThread
3. Appelle `do_execve(thread, pcb, path, &[], &[])`
4. En cas de succès : met à jour `frame.rcx = entry_point`, `frame.rsp = initial_rsp`,
   `frame.r11 = 0x0202`, `frame.rax = 0`, `gs:[0x08] = initial_rsp`
5. SYSRETQ saute directement au nouveau point d'entrée ELF

> **Note** : argv et envp sont passés vides pour l'instant (ElfLoader gère la pile initiale).
> Le câblage complet de argv/envp userspace est en Phase 4.

---

## 4. Livraison des signaux ✅

**Fichier** : `kernel/src/syscall/dispatch.rs` — `check_and_deliver_signals()`

Câblage complet via conversion arch `SyscallFrame` ↔ delivery `SyscallFrame` :

```rust
// arch::SyscallFrame → delivery::SyscallFrame
let mut d_frame = DeliveryFrame {
    user_rip:    frame.rcx,    // RIP retour (sauvé par SYSCALL hw)
    user_rflags: frame.r11,    // RFLAGS (sauvé par SYSCALL hw)
    user_rsp:    frame.rsp,    // RSP userspace
    user_rax:    frame.rax,    // valeur de retour syscall
    ...
};
handle_pending_signals(thread, &mut d_frame);
// Copie retour : d_frame → frame (RIP, RSP, RFLAGS, rax modifiés si handler installé)
```

La livraison modifie `frame.rcx` (nouveau RIP = handler signal) et `frame.rsp`
(nouveau RSP = sig_frame sur stack) si un handler userspace est installé.

---

## 5. État des syscalls Phase 3

| Syscall | N° Linux | État | Notes |
|---|---|---|---|
| `sys_rt_sigaction` | 13 | ✅ Implémenté | Lecture/écriture PCB.sig_handlers |
| `sys_rt_sigprocmask` | 14 | ✅ Implémenté | SIGKILL/SIGSTOP non-masquables |
| `sys_rt_sigreturn` | 15 | ✅ Implémenté | Magic SIG-13 vérifié constant-time |
| `sys_fork` | 57 | ✅ Implémenté | fork_child_trampoline + iretq |
| `sys_execve` | 59 | ✅ Implémenté | do_execve() + argv/envp copié depuis userspace (ARGV-01) |
| `sys_exit` | 60 | ✅ Implémenté | PCB Zombie + schedule_block |
| `sys_wait4` | 61 | ✅ Implémenté | → do_waitpid() ; WNOHANG=0, ECHILD, EINTR |
| `sys_waitid` | 247 | ✅ Implémenté | → do_waitpid() ; siginfo_t x86_64 rempli |
| `sys_clone` | 56 | ✅ Implémenté | → create_thread() ; CLONE_VM/THREAD/SIGHAND |
| `sys_kill` | 62 | ✅ Implémenté | → send_signal_to_pid() |
| `sys_tgkill` | 234 | ✅ Implémenté | → send_signal_to_tcb() ; SigInfo::from_kill |
| `sys_sigaltstack` | 131 | ✅ Implémenté | thread.addresses.sigaltstack_base/size |
| `sys_uname` | 63 | ✅ Implémenté | struct utsname 390 bytes ; "Exo-OS" x86_64 |
| Signal delivery | — | ✅ Câblé | post_dispatch → check_and_deliver_signals |

---

## 6. Phase 3 — COMPLÈTE ✅

Tous les items Phase 3 ont été implémentés et `cargo check` passe sans erreur.

### Changements apportés (session de complétion)

| Item | Fichier | Description |
|---|---|---|
| WAIT-01 | `handlers/process.rs` | `sys_wait4` → `do_waitpid()` ; wstatus en userspace ; WNOHANG→0 |
| WAIT-01 | `handlers/process.rs` | `sys_waitid` → `do_waitpid()` ; siginfo_t layout x86_64 |
| KILL-01 | `handlers/signal.rs` | `sys_kill` → `send_signal_to_pid()` ; pid<0 ESRCH |
| KILL-02 | `handlers/signal.rs` | `sys_tgkill` → `send_signal_to_tcb()` ; SigInfo::from_kill |
| ALTSTACK | `handlers/signal.rs` | `sys_sigaltstack` → lecture/écriture `thread.addresses.sigaltstack_*` |
| UNAME | `handlers/misc.rs` | `sys_uname` → `struct utsname` 390 bytes ; Exo-OS/x86_64 |
| ARGV-01 | `syscall/dispatch.rs` | `copy_userspace_argv()` ; argv/envp copiés avant `do_execve()` |
| errno | `syscall/errno.rs` | `ESRCH=-3`, `ECHILD=-10` ajoutés |

---

## 7. Résumé des fichiers modifiés en Phase 3

| Fichier | Modifications |
|---|---|
| `kernel/src/process/signal/handler.rs` | SIG-13 : magic SIGNAL_FRAME_MAGIC, verify_and_extract_uc() |
| `kernel/src/process/lifecycle/fork.rs` | Kernel stack fils 96 bytes, fork_child_trampoline |
| `kernel/src/scheduler/asm/switch_asm.s` | fork_child_trampoline : xor rax + swapgs + iretq |
| `kernel/src/syscall/dispatch.rs` | handle_sigreturn_inplace, handle_fork_inplace, handle_execve_inplace, check_and_deliver_signals, copy_userspace_argv (ARGV-01) |
| `kernel/src/syscall/handlers/signal.rs` | sys_rt_sigaction, sys_rt_sigprocmask, sys_kill, sys_tgkill, sys_sigaltstack |
| `kernel/src/syscall/handlers/process.rs` | sys_wait4, sys_waitid |
| `kernel/src/syscall/handlers/misc.rs` | sys_uname |
| `kernel/src/syscall/errno.rs` | ESRCH=-3, ECHILD=-10 |
| `docs/Phases/PHASE2_SCHEDULER_IPC.md` | SWAPGS ✅, ERR-01 ✅ |
| `docs/Phases/ExoOS_Roadmap_Avant_ExoBoot.md` | Phase 1 et 2 tous ✅ |
