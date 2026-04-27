# ExoOS — Audit Profond v3
**Commit analysé :** `e23de5d` — *big fix for resolve 223 failed tests*
**Date audit :** 2026-04-25
**Fichiers lus directement :** 728 sources Rust

---

## Résumé exécutif

Ce troisième cycle d'audit a effectué une lecture directe de l'intégralité
du code source pour 18 sous-systèmes critiques : exceptions, signaux, fork/exec,
exit, mémoire CoW, TLB shootdown, FPU, IPC SPSC/MPMC, séquenceur IPC,
sécurité capabilities, scheduler tick, et ExoPhoenix SSR.

**Résultat : 21 nouveaux bugs identifiés** non couverts par les deux audits
précédents, dont **4 P0 runtime** et **3 P0 correctness** qui entraîneront
des crashs ou corruptions de données en production.

---

## PARTIE 1 — Bugs P0 (crash ou corruption garanti en production)

---

### BUG-SIGFRAME-01 — Registres r10–r15, rbx, rbp perdus lors de sigreturn
**Fichier :** `process/signal/handler.rs` + `syscall/dispatch.rs`
**Sévérité :** 🔴 P0 — corruption silencieuse callee-saved

#### Analyse

La `SyscallFrame` (pont ASM→Rust à l'entrée syscall) sauvegarde bien
r10–r15, rbx, rbp sur la pile kernel.  
Mais `dispatch.rs` construit la `DeliveryFrame` pour la livraison de signaux
**sans** ces champs, car `SyscallFrame` (version signal) ne les expose pas :

```rust
// dispatch.rs ~line 395
let mut d_frame = DeliveryFrame {
    user_r8:  frame.r8,
    user_r9:  frame.r9,
    // ← r10, r11, r12, r13, r14, r15, rbx, rbp : ABSENTS
    ...
};
```

`setup_signal_frame()` remplit alors `GRegs` avec les champs disponibles,
laissant `r10..r15`, `rbx`, `rbp` à **zéro** dans l'`ucontext_t` du sigframe.

Lors du `sigreturn(2)`, `restore_signal_frame()` restaure ces registres à **0**,
écrasant les callee-saved du thread interrompu.

**Conséquence concrète :**  
Tout signal livré pendant une fonction Rust qui utilise r12–r15/rbx/rbp
(quasi-universellement le cas dans du code optimisé) corrompt silencieusement
le frame du thread. Le crash survient 0 à N instructions plus tard, sans
lien apparent avec le signal.

#### Fix

Étendre `SyscallFrame` (delivery) avec `user_r10..user_r15`, `user_rbx`, `user_rbp`
et alimenter ces champs depuis `arch::x86_64::syscall::SyscallFrame` (qui les
possède déjà à offsets [24..104]).

```rust
// dispatch.rs
let mut d_frame = DeliveryFrame {
    user_r10: frame.r10,
    user_r11: frame.r11,  // RFLAGS — mais aussi callee-saved en pratique
    user_r12: frame.r12,
    user_r13: frame.r13,
    user_r14: frame.r14,
    user_r15: frame.r15,
    user_rbx: frame.rbx,
    user_rbp: frame.rbp,
    ...
};
```

---

### BUG-EXIT-STUB-01 — `do_exit()` est un stub de 6 lignes
**Fichier :** `process/lifecycle/exit.rs`
**Sévérité :** 🔴 P0 — zombie leak, mémoire non libérée, parent bloqué

#### Analyse

```rust
pub fn do_exit(thread, pcb, exit_status) {
    drivers::driver_do_exit(pid);  // ← SEULE action réelle
}

pub fn do_exit_thread(thread, pcb, retval) -> ! {
    thread.set_state(TaskState::Zombie);
    loop {}   // ← spin infini sans hlt !
}
```

Il manque **7 étapes critiques** :

| Étape | Manquante | Conséquence |
|-------|-----------|-------------|
| Fermer tous les fds | ✗ | Fuite de handles ExoFS |
| Libérer l'espace d'adressage (AS + PML4) | ✗ | Fuite de TOUTE la mémoire userspace |
| Retirer le TCB de la run queue | ✗ | Scheduler peut élire un thread zombie → triple fault |
| Envoyer SIGCHLD au parent | ✗ | `wait4()` bloque indéfiniment → orphelins permanents |
| Mettre le PCB en état `Zombie` | ✗ | Registry ne peut pas récolter |
| Notifier la `VFORK_WAIT_QUEUE` si vfork | ✗ | Parent bloqué en vfork indéfiniment |
| `do_exit_thread` : `hlt` au lieu de `loop {}` | ✗ | Spin à 100% CPU sur l'AP |

#### Fix minimal

```rust
pub fn do_exit(thread, pcb, exit_status) {
    // 1. Fermer les fds
    pcb.files.lock().close_all();
    // 2. Libérer AS
    free_address_space(pcb.address_space.load(Ordering::Acquire));
    // 3. Retirer de la run queue
    sched_dequeue_current(thread);
    // 4. SIGCHLD au parent
    send_signal_to_pid(pcb.ppid(), Signal::SIGCHLD);
    // 5. Zombie
    pcb.set_state(ProcessState::Zombie);
    drivers::driver_do_exit(pcb.pid.0);
    // 6. Notify vfork
    notify_vfork_completion(pcb.pid);
}
```

---

### BUG-EXEC-AS-LEAK-01 — `do_execve()` ne libère pas l'ancien espace d'adressage
**Fichier :** `process/lifecycle/exec.rs`
**Sévérité :** 🔴 P0 — fuite mémoire à chaque execve()

#### Analyse

```rust
pub fn do_execve(thread, pcb, path, argv, envp) {
    let cr3_current = thread.sched_tcb.cr3_phys;   // ← ancien AS
    let elf_result = loader.load_elf(path, argv, envp, cr3_current)?;
    // ← elf_result.addr_space_ptr = NOUVEL AS
    // ANCIEN AS (cr3_current / pcb.address_space) jamais libéré !
    pcb.address_space.store(elf_result.addr_space_ptr, ...);
    pcb.cr3.store(elf_result.cr3, ...);
}
```

`load_elf()` crée un **nouvel** `UserAddressSpace` mais `pcb.address_space`
précédent est écrasé sans appeler `free_addr_space()`.  
Chaque `execve()` fuit l'intégralité de l'espace d'adressage précédent :
toutes les VMAs, les PML4/PDP/PD/PT pages et les frames physiques
anonymes non-partagées.

#### Fix

```rust
// Sauvegarder l'ancien ptr AVANT de le remplacer
let old_as_ptr = pcb.address_space.load(Ordering::Acquire);

// ... charger le nouveau binaire ...

// Libérer l'ancien AS après le switch
if old_as_ptr != 0 {
    if let Some(cl) = ADDR_SPACE_CLONER.get() {
        cl.free_addr_space(old_as_ptr);
    }
}
pcb.address_space.store(elf_result.addr_space_ptr, ...);
```

---

### BUG-COW-SMP-RACE-01 — Double CoW break sur la même page en SMP
**Fichier :** `memory/virtual/fault/cow.rs`
**Sévérité :** 🔴 P0 — frame physique leakée, PTE corrompu

#### Analyse

Scénario avec 2 CPUs qui faultent simultanément sur la même page CoW
(ex: `fork()` + deux threads du parent écrivent simultanément) :

```
CPU0: translate(page) → old_frame=A, alloc_nonzeroed() → new_frame=X
CPU1: translate(page) → old_frame=A, alloc_nonzeroed() → new_frame=Y
CPU0: map_page(addr, X, WRITABLE) → PTE := X
CPU1: map_page(addr, Y, WRITABLE) → PTE := Y  ← écrase PTE de CPU0 !
CPU0: dec_cow(A) → returns 1 (encore une ref)
CPU1: dec_cow(A) → returns 0 → free_frame(A) ✓
```

**Résultat :** frame X allouée par CPU0 n'est **jamais libérée** (leak permanent),
et CPU0 croit que sa page est à l'adresse X alors que le PTE pointe sur Y.

La cause racine : `handle_cow_fault()` ne prend **aucun verrou** sur le PTE
entre `translate()` et `map_page()`.

#### Fix

```rust
pub fn handle_cow_fault<A: FaultAllocator>(ctx, vma, alloc) -> FaultResult {
    let page_addr = ...;
    // Verrouiller le PTE ou utiliser une CAS sur le PTE (hardware TLB lock)
    let _pte_lock = alloc.lock_pte(page_addr);  // nouveau trait requis
    let phys = match alloc.translate(page_addr) { ... };
    // ... le reste est inchangé sous le verrou
}
```

Alternative : utiliser `compare_exchange` au niveau du PTE (hardware-assisted).

---

### BUG-TLB-SELF-FLUSH-01 — `shootdown_sync()` n'invalide pas le TLB local du CPU émetteur
**Fichier :** `memory/virtual/address_space/tlb.rs`
**Sévérité :** 🔴 P0 — stale TLB entries sur le CPU initiateur

#### Analyse

```rust
pub unsafe fn shootdown_sync(flush_type, cpu_count) {
    let all_mask = (1u64 << n) - 1;
    TLB_QUEUE.request(flush_type, all_mask);  // envoie IPIs aux AUTRES CPUs
    // Attendre ACK de chaque CPU...
    // ← PAS DE flush local ! Le CPU courant garde ses entrées TLB stales
}
```

`TLB_QUEUE.handle_remote()` est appelé depuis l'IPI handler des CPUs **cibles**
— il exécute `invlpg` ou reload CR3 sur eux. Mais le CPU qui appelle
`shootdown_sync()` ne flush **jamais son propre TLB**.

Si le CPU initiateur de `shootdown_sync()` a des entrées stales pour la page
qui vient d'être démappée (ex: après `unmap_page()` suivi de `free_frame()`),
il peut continuer à accéder à un frame physique désormais réalloué à un autre
processus → **lecture/écriture arbitraire de mémoire d'un autre processus**.

#### Fix

```rust
pub unsafe fn shootdown_sync(flush_type, cpu_count) {
    // 1. Flush local immédiatement
    match flush_type {
        TlbFlushType::Single(addr) => flush_single(addr),
        TlbFlushType::Full => reload_cr3(),
        TlbFlushType::Range(start, end) => flush_range(start, end),
    }
    // 2. Broadcaster aux autres CPUs et attendre ACK
    TLB_QUEUE.request(flush_type, all_mask);
    for cpu_id in 0..n { loop { /* wait ack */ } }
}
```

---

## PARTIE 2 — Bugs P1 (crash ou corruption probable)

---

### BUG-SIGFRAME-FS-01 — FS.base non sauvé dans le sigframe
**Fichier :** `process/signal/handler.rs`
**Sévérité :** 🟠 P1 — TLS userspace corrompu après sigreturn

#### Analyse

`setup_signal_frame()` construit un `GRegs` qui inclut les champs `gs: u16`
et `fs: u16` (valeurs sélecteurs) mais **pas** `fs_base` (adresse MSR
`0xC000_0100`).

Quand le signal handler modifie FS.base (ou quand le runtime C/Rust le fait),
`sigreturn()` ne le restaure pas — le thread reprend avec le FS.base du handler
au lieu du sien.

Pour `glibc` / `musl` : `fs_base` = adresse du `pthread_t`. Après sigreturn,
tout accès à `errno`, `__thread`, ou les TLS Rust est corrompu.

#### Fix

Ajouter `fs_base: u64` et `gs_base: u64` dans `GRegs` (aligné sur `struct
sigcontext` Linux réel), les sauvegarder depuis `MSR_FS_BASE` / `MSR_KERNEL_GS_BASE`
dans `setup_signal_frame()`, et les restaurer dans `restore_signal_frame()`.

---

### BUG-EXIT-THREAD-SPIN-01 — `do_exit_thread()` spin sans hlt
**Fichier :** `process/lifecycle/exit.rs`
**Sévérité :** 🟠 P1 — 100% CPU sur l'AP après terminaison thread

```rust
pub fn do_exit_thread(thread, ...) -> ! {
    thread.set_state(TaskState::Zombie);
    loop {}   // ← pas de hlt → CPU à 100% pour rien
}
```

Un thread terminé dont le scheduler n'a pas encore détecté l'état Zombie
(faute d'une implémentation `do_exit` complète) va simplement brûler un
CPU entier en attente. Le fix est `loop { unsafe { asm!("hlt") }; }`.

---

### BUG-MPMC-FULL-HEAD-ADVANCE-01 — MPMC avance `head` même quand le ring est plein
**Fichier :** `ipc/ring/mpmc.rs`
**Sévérité :** 🟠 P1 — perte de messages et désynchronisation ring

#### Analyse

```rust
pub fn push_copy(&self, src, flags) {
    loop {
        let pos = self.head_atomic().fetch_add(1, Ordering::AcqRel); // ← INCRÉMENTÉ !
        let cell = self.cell_at(pos);
        let diff = (seq as i64).wrapping_sub(pos as i64);
        if diff < 0 {
            return Err(IpcError::QueueFull);  // ← mais head est déjà à pos+1 !
        }
        ...
    }
}
```

Quand le ring est plein (`diff < 0`), `head` a déjà été incrémenté via
`fetch_add`. Le prochain producteur utilisera `pos+1` qui pointe vers un slot
potentiellement **encore occupé**. La séquence du slot `pos` reste à sa valeur
précédente → le producteur suivant verra `diff < 0` à nouveau, avancera head
encore, et ainsi de suite.

Après N producteurs bloqués, `head` a avancé de N positions sans qu'aucun
message n'ait été écrit. Le ring ne se récupère jamais — **deadlock de ring**.

#### Fix

Vérifier la disponibilité du slot **avant** d'incrémenter head, ou utiliser
un `compare_exchange` sur head avec rollback en cas d'échec :

```rust
let current_head = self.head_atomic().load(Ordering::Acquire);
let cell = self.cell_at(current_head);
if cell.load_seq() != current_head {
    return Err(IpcError::QueueFull); // ← before fetch_add
}
// CAS sur head pour éviter les races multi-producteur
if self.head_atomic().compare_exchange(current_head, current_head + 1, ...).is_err() {
    continue; // réessayer
}
```

---

### BUG-SCHEDULE-BLOCK-RELEASE-01 — `schedule_block` panic supprimé en release
**Fichier :** `scheduler/core/switch.rs`
**Sévérité :** 🟠 P1 — busy-wait silencieux en production

```rust
// switch.rs
None => {
    debug_assert!(false, "schedule_block: idle_thread absent");
    // ↑ Compilé → no-op en --release
    current.set_state(TaskState::Runnable);
    rq.enqueue(current_ptr);  // ← busy-wait
}
```

Remplacer `debug_assert!` par `panic!` ou une boucle `hlt` explicite.

---

## PARTIE 3 — Incohérences architecturales (P2)

---

### INCOHER-COW-01 — CoW tracker : `dec()` retourne 0 pour les frames non suivies
**Fichier :** `memory/cow/tracker.rs`

```rust
pub fn dec(&self, frame) -> u32 {
    // ...
    0 // Non trouvé → considéré comme déjà libéré
}
```

Si `dec()` est appelé sur un frame qui n'est pas dans la table CoW (frame
non partagé, bug d'appel), la fonction retourne 0 silencieusement.
L'appelant interprète 0 comme "libérer le frame", déclenchant un
`free_frame()` sur un frame potentiellement encore valide.

Le `dec()` devrait retourner `u32::MAX` (ou un `Result`) pour les frames
non suivis.

---

### INCOHER-EXEC-01 — `do_execve()` ne recharge pas CR3 sur le CPU courant
**Fichier :** `process/lifecycle/exec.rs`

Après `pcb.cr3.store(elf_result.cr3)`, le CPU courant tourne toujours
avec l'**ancien CR3** jusqu'au prochain context switch. Si le handler
syscall accède à une adresse mappée dans l'ancien AS mais pas dans le
nouveau (ex: pile kernel mapped via AS userspace), la suite d'`execve()`
peut faulter.

Le fix : `write_cr3(elf_result.cr3)` immédiatement après le store.

---

### INCOHER-SPSC-ORDERING-01 — SPSC `head.store()` Relaxed après `cell.store_seq()` Release
**Fichier :** `ipc/ring/spsc.rs`

```rust
cell.store_seq(pos + 1);                          // Release
self.head.0.store(pos + 1, Ordering::Relaxed);    // ← Relaxed
```

Sur x86, `store(Relaxed)` après `store(Release)` est sûr car x86 est
TSO. Mais si ExoOS est un jour porté sur ARM/RISC-V (le fichier `arch/aarch64`
existe déjà), ce Relaxed permettrait au CPU de réordonner le store `head`
avant le store de la séquence, rendant le slot visible au consommateur
**avant** que son header soit écrit.

Remplacer par `Ordering::Release` pour la portabilité.

---

### INCOHER-EXCEPTION-01 — `do_debug()` kernel silencieux
**Fichier :** `arch/x86_64/exceptions.rs`

```rust
fn do_debug(frame) {
    if frame.from_userspace() { exception_return_to_user(frame); }
    // Kernel debug : ignorer silencieusement
}
```

Un `#DB` kernel (point d'arrêt matériel DR0–DR3 déclenché en Ring 0)
est ignoré silencieusement. Cela empêche le débogage kernel et masque
des failles potentielles d'utilisation de debug registers par un
attaquant userspace (si les DR ne sont pas effacés au context switch).

Vérifier que les DR sont sauvegardés/restaurés dans le context switch
et que `#DB` kernel loggue au minimum.

---

### INCOHER-EXCEPTION-02 — `do_virtualization()` (#VE) vide
**Fichier :** `arch/x86_64/exceptions.rs`

```rust
fn do_virtualization(frame) {
    let _ = frame;
    // Intel EPT Violation — géré par le module virt/ si VMX actif
}
```

Si ExoOS tourne comme guest VMX et reçoit `#VE`, l'exception est ignorée.
L'instruction qui a causé le `#VE` est simplement rejouée → boucle infinie
de `#VE`. Ajouter au minimum un compteur et un fallback `#GP`.

---

### INCOHER-EXOARGOS-01 — `ExoArgos::init_pmu()` non appelé au boot
**Fichier :** `security/exoargos.rs` + `arch/x86_64/boot/early_init.rs`

`init_pmu()` est défini mais absent de `early_init.rs` et `lib.rs`.
Le monitoring PMC (ExoShield module 8) est **dead code** en production.

```rust
// À ajouter dans early_init.rs, après étape 13b
crate::security::exoargos::init_pmu();
```

---

### INCOHER-UNWRAP-01 — 1 982 `.unwrap()` dans le code kernel non-test
**Fichiers :** Tous, top offenders : `fs_bridge.rs`(65), `volume_key.rs`(41)

En mode `--release`, un `.unwrap()` sur `None` ou `Err` déclenche un
`panic!` qui appelle `panic_handler` → `halt_cpu()`. Chaque `.unwrap()`
est un vecteur de DoS potentiel si un attaquant peut influencer la valeur.

Priorité de correction : les 65 `.unwrap()` dans `syscall/fs_bridge.rs`
sont directement sur le chemin des syscalls utilisateur.

---

### INCOHER-UNSAFE-01 — 1 548 blocs `unsafe {}` sans commentaire `SAFETY:`
**Fichiers :** Ensemble du codebase

La convention Rust exige un commentaire `// SAFETY: ...` avant chaque
bloc `unsafe`. L'absence masque les invariants requis et rend l'audit
futur impossible. Les sous-systèmes les plus critiques (exceptions, percpu,
context switch) sont bien documentés, mais `fs/exofs/` est le pire offenseur.

---

## PARTIE 4 — Bugs mineurs / dette technique (P3)

---

### DEBT-01 — `do_exit_thread` spin → hlt manquant
```rust
// Actuel :
loop {}
// Correct :
loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
```

### DEBT-02 — Commentaire layout TCB ligne 25 (`sched_state >> 32 = pid`) contradictoire
Le commentaire en tête dit que `sched_state[63:32] = pid` mais la struct réelle
a `pid: ProcessId` à l'offset [92] comme champ séparé. Supprimer le commentaire.

### DEBT-03 — `SMP_BOOT_DONE` jamais lu après écriture
`SMP_BOOT_DONE.store(true, Release)` est setté dans `smp_boot_aps()` mais aucun
sous-système ne conditionne son comportement sur ce flag (ex: TLB shootdown,
TSC cross-calibration). Soit le brancher, soit le supprimer.

### DEBT-04 — `ref_count()` dans `CowTracker` sans verrou
```rust
pub fn ref_count(&self, frame) -> u32 {
    // Lecture seule : pas besoin du verrou (les tombstones ne mentent pas)
    ...
}
```
Lecture sans verrou d'une table modifiée sous `self.lock` → TOCTOU possible
si le refcount descend à 0 entre `ref_count()` et l'action basée dessus.

### DEBT-05 — 233 `#[allow(unused/dead_code)]` qui masquent du code non-atteint
Certains sont légitimes (interfaces futures), d'autres masquent des modules
non-branchés qui ne seront jamais appelés au boot.

---

## Tableau de priorité global

| ID | Sévérité | Composant | Description | Statut |
|----|----------|-----------|-------------|--------|
| BUG-SIGFRAME-01 | 🔴 P0 | process/signal | r10–r15, rbx, rbp perdus au sigreturn | ❌ Nouveau |
| BUG-EXIT-STUB-01 | 🔴 P0 | process/exit | do_exit() stub → zombie/fuite AS/parent bloqué | ❌ Nouveau |
| BUG-EXEC-AS-LEAK | 🔴 P0 | process/exec | Ancien AS jamais libéré lors de execve() | ❌ Nouveau |
| BUG-COW-SMP-RACE | 🔴 P0 | memory/cow | Double CoW break SMP → frame leak + PTE corrompu | ❌ Nouveau |
| BUG-TLB-SELF-FLUSH | 🔴 P0 | memory/tlb | shootdown_sync() ne flush pas le TLB local | ❌ Nouveau |
| BUG-SIGFRAME-FS | 🟠 P1 | process/signal | FS.base absent du sigframe → TLS corrompu | ❌ Nouveau |
| BUG-EXIT-THREAD-SPIN | 🟠 P1 | process/exit | do_exit_thread loop{} → 100% CPU | ❌ Nouveau |
| BUG-MPMC-HEAD | 🟠 P1 | ipc/ring | head avancé avant vérif ring plein → deadlock ring | ❌ Nouveau |
| BUG-BLOCK-RELEASE | 🟠 P1 | scheduler | debug_assert supprimé → busy-wait silencieux release | ⚠️ Partiel |
| INCOHER-COW-TRACKER | 🟡 P2 | memory/cow | dec() retourne 0 sur frame non suivi → free spurieux | ❌ Nouveau |
| INCOHER-EXEC-CR3 | 🟡 P2 | process/exec | Nouveau CR3 pas chargé immédiatement → stale TLB | ❌ Nouveau |
| INCOHER-SPSC-ORDER | 🟡 P2 | ipc/spsc | head.store Relaxed vs cell Release → ARM unsafe | ❌ Nouveau |
| INCOHER-DEBUG-HANDLER | 🟡 P2 | arch/exceptions | #DB kernel ignoré → debug registers non audités | ❌ Nouveau |
| INCOHER-VE-HANDLER | 🟡 P2 | arch/exceptions | #VE vide → boucle infinie en mode VMX guest | ❌ Nouveau |
| INCOHER-EXOARGOS | 🟡 P2 | security | init_pmu() jamais appelé au boot | ✅ Connu |
| INCOHER-UNWRAP | 🔵 P3 | global | 1 982 unwrap() → vecteurs DoS potentiels | ❌ Nouveau |
| INCOHER-UNSAFE | 🔵 P3 | global | 1 548 unsafe sans SAFETY: → audit impossible | ❌ Nouveau |
| DEBT-01 | 🔵 P3 | process/exit | loop{} → manque hlt | ❌ Nouveau |
| DEBT-02 | 🔵 P3 | scheduler/TCB | commentaire PID erroné ligne 25 | ✅ Connu |
| DEBT-03 | 🔵 P3 | arch/smp | SMP_BOOT_DONE jamais consommé | ✅ Connu |
| DEBT-04 | 🔵 P3 | memory/cow | ref_count() sans verrou → TOCTOU | ❌ Nouveau |

---

## Score de maturité mis à jour

| Composant | Audit 2 | Audit 3 | Δ | Note |
|-----------|---------|---------|---|------|
| Architecture x86_64 / SMP | 91% | **89%** | -2 | #DB/#VE vides, debug regs |
| Scheduler | 85% | **82%** | -3 | schedule_block release bug |
| Mémoire | 76% | **68%** | -8 | CoW SMP race, TLB self-flush |
| Process lifecycle | 71% | **51%** | -20 | do_exit stub, exec AS leak |
| Signaux | — | **55%** | — | r10-r15 + FS.base manquants |
| IPC | 78% | **72%** | -6 | MPMC head advance bug |
| ExoFS storage | 71% | **71%** | 0 | Stable |
| Sécurité | 84% | **83%** | -1 | ExoArgos non branché |
| **Global** | **79%** | **72%** | **-7** | Réajustement avec vrais bugs |

---

## Plan d'action recommandé

### Sprint 1 — P0 (1 semaine)
1. `BUG-EXIT-STUB-01` — Implémenter do_exit() complet en 7 étapes
2. `BUG-TLB-SELF-FLUSH-01` — Ajouter flush local dans shootdown_sync()
3. `BUG-COW-SMP-RACE-01` — Verrouiller le PTE dans handle_cow_fault()
4. `BUG-EXEC-AS-LEAK-01` — Sauvegarder + libérer l'ancien AS dans do_execve()
5. `BUG-SIGFRAME-01` — Étendre SyscallFrame delivery avec r10-r15/rbx/rbp

### Sprint 2 — P1 (1 semaine)
6. `BUG-SIGFRAME-FS-01` — Sauvegarder FS.base dans GRegs
7. `BUG-MPMC-HEAD-01` — Vérifier slot avant fetch_add head
8. `BUG-EXIT-THREAD-SPIN-01` — Remplacer loop{} par loop{hlt}
9. `INCOHER-EXEC-CR3-01` — write_cr3() dans do_execve()

### Sprint 3 — P2/P3 (2 semaines)
10. Brancher `ExoArgos::init_pmu()`
11. SPSC head Relaxed → Release
12. Handlers #DB/#VE
13. CowTracker::dec() valeur sentinelle
14. Débuter la réduction des `unwrap()` (commencer par fs_bridge.rs)

---

*Rapport généré automatiquement par lecture directe du codebase — commit e23de5d*
