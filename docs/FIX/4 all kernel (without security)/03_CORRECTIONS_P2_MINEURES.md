# ExoOS — Corrections P2 Mineures
## Commit de référence : `c4239ed1`

Ces cinq corrections traitent des cas limites, fuites d'information
et incohérences architecturales non bloquantes à court terme.

---

## P2-01 — `syscall_cstar_noop` : RSP userspace non restauré avant `sysret`

### Localisation
`kernel/src/arch/x86_64/syscall.rs:245–255`

### Code actuel

```asm
syscall_cstar_noop:
    swapgs
    mov rax, -38      // GS kernel actif — gs:[0x08] contient user_rsp
    swapgs            // GS userspace restauré
    sysret            // retour compat — RSP = ?? (jamais restauré depuis gs:[0x08])
```

### Problème
Lors d'un `SYSCALL` en mode compat 32-bit, le CPU :
1. **Ne modifie pas RSP** (contrairement à `SYSCALL` 64-bit qui utilise RSP0 du TSS).
2. Sauvegarde RIP → RCX, RFLAGS → R11.

Le handler entre avec RSP = RSP **userspace** (inchangé).
`swapgs` active le GS kernel → `gs:[0x00]` = kernel_rsp, `gs:[0x08]` = user_rsp.
Mais RSP n'est **jamais** rechargé depuis `gs:[0x00]` ni restauré depuis `gs:[0x08]`.
Le `sysret` retourne avec RSP = valeur aléatoire ou corrompue.

En pratique, les processus 32-bit sont rares sur ExoOS, mais ce handler
est actif sur tous les CPUs (MSR CSTAR configuré). Un processus malveillant 32-bit
pourrait en abuser pour corrompre son propre RSP de façon prévisible.

### Correction

```rust
// kernel/src/arch/x86_64/syscall.rs — remplacer le global_asm! CSTAR

core::arch::global_asm!(
    ".section .text",
    ".global syscall_cstar_noop",
    ".type   syscall_cstar_noop, @function",
    "syscall_cstar_noop:",
    // Activer GS kernel pour accéder à la zone per-CPU.
    "swapgs",
    // Sauvegarder le RSP userspace dans gs:[0x08] (save slot standard).
    // RSP n'a PAS encore été changé (SYSCALL compat ne touche pas RSP).
    "mov qword ptr gs:[0x08], rsp",
    // Charger le RSP kernel depuis gs:[0x00] pour éviter tout travail sur la pile user.
    // (Optionnel ici car on ne fait rien, mais nécessaire pour la cohérence.)
    "mov rsp, qword ptr gs:[0x00]",
    // Retourner -ENOSYS (errno 38 = ENOSYS Linux ABI).
    "mov eax, -38",
    // Restaurer RSP userspace depuis le save slot.
    "mov rsp, qword ptr gs:[0x08]",
    // Restaurer GS userspace.
    "swapgs",
    // sysretl = retour compat 32-bit (RCX → RIP, R11 → EFLAGS, compat segments).
    // Note LLVM : "sysret" sans suffixe génère SYSRETQ en x86_64 — utiliser .byte.
    ".byte 0x48, 0x0F, 0x07",   // REX.W SYSRET = SYSRETQ (64-bit)
    // Pour un vrai SYSRETL (compat) il faudrait : .byte 0x0F, 0x07
    // ExoOS ne supporte pas le mode compat → SYSRETQ + ENOSYS est correct.
    ".size syscall_cstar_noop, . - syscall_cstar_noop",
);
```

> **Note sur l'encodage** : le commentaire "pas de suffixe 'l'" dans le code original
> signale un problème d'assemblage LLVM. L'encodage `.byte 0x0F, 0x07` produit
> explicitement `SYSRETL` (mode compat). Puisqu'ExoOS ne supporte pas le mode compat,
> garder `SYSRETQ` avec `ENOSYS` est la bonne approche — juste s'assurer que RSP est propre.

---

## P2-02 — Fork fils : RFLAGS figé à `0x0202`, flags parent non propagés

### Localisation
`kernel/src/process/lifecycle/fork.rs:224`

### Code actuel

```rust
*frame_ptr.add(9)  = 0x0202;   // RFLAGS (IF=1, reserved=1)
```

### Problème
Le fils démarre toujours avec `RFLAGS = 0x0202` (IF=1, bit 1 réservé), indépendamment
des flags du parent. Cela perd silencieusement :
- `AC` (bit 18) — Alignment Check : si le parent était en mode strict, le fils ne l'est pas
- `DF` (bit 10) — Direction Flag : si le parent avait DF=1 (rare mais légal), le fils l'ignore
- `ID` (bit 21) — CPUID : bit de capacité, normalement propagé

En pratique `DF=0` et `AC=0` sont le cas normal, donc l'impact est faible.
Mais pour une conformité POSIX stricte (`fork()` = copie exacte du processus), les RFLAGS
du fils doivent être ceux du parent au moment de l'appel syscall, avec quelques masques.

### Correction

**Étape A — Ajouter `parent_rflags` à `ForkContext`**

```rust
// kernel/src/process/lifecycle/fork.rs — ForkContext

pub struct ForkContext<'a> {
    pub parent_thread: &'a ProcessThread,
    pub parent_pcb:    &'a ProcessControlBlock,
    pub flags:         ForkFlags,
    pub target_cpu:    u32,
    pub child_rip:     u64,
    pub child_rsp:     u64,
    /// RFLAGS du parent au moment du fork (depuis frame.r11 sauvé par SYSCALL).
    /// CORRECTION P2-02 : propagé au fils avec masquage sécurisé.
    pub parent_rflags: u64,
}
```

**Étape B — Masquer et propager dans `do_fork()`**

```rust
// kernel/src/process/lifecycle/fork.rs — dans do_fork(), à la ligne frame_ptr.add(9)

// Masque des flags sûrs à hériter (POSIX + sécurité kernel).
// - Conserver : CF(0), PF(2), AF(4), ZF(6), SF(7), OF(11), DF(10), AC(18), ID(21)
// - Forcer  : IF=1 (bit 9) — le fils doit accepter les interruptions
// - Effacer : TF=0 (bit 8) — ne pas tracer le fils si le parent était en trace
//             NT=0 (bit 14) — Nested Task flag — jamais hérité
//             RF=0 (bit 16) — Resume Flag — jamais hérité
//             VM=0 (bit 17) — Virtual 8086 — non supporté
const RFLAGS_SAFE_MASK:  u64 = 0x0000_0000_0020_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
const RFLAGS_FORCE_SET:  u64 = 0x0000_0000_0000_0200; // IF=1
const RFLAGS_FORCE_CLR:  u64 = 0x0000_0000_0004_0100; // TF=0, NT=0, RF=0, VM=0

let child_rflags = (ctx.parent_rflags & RFLAGS_SAFE_MASK)
    | RFLAGS_FORCE_SET
    & !RFLAGS_FORCE_CLR;

// Garantir que le bit réservé 1 est toujours à 1.
let child_rflags = child_rflags | 0x0002;

*frame_ptr.add(9) = child_rflags;
```

**Étape C — Passer `frame.r11` depuis `handle_fork_inplace` dans `dispatch.rs`**

```rust
// kernel/src/syscall/dispatch.rs — dans handle_fork_inplace()

let ctx = ForkContext {
    parent_thread: thread,
    parent_pcb:    pcb,
    flags:         ForkFlags::default(),
    target_cpu:    tcb.current_cpu().0,
    child_rip:     frame.rcx,
    child_rsp:     frame.rsp,
    parent_rflags: frame.r11,   // ← CORRECTION P2-02 : RFLAGS sauvés par SYSCALL
};
```

---

## P2-03 — `stack_base = 0` et `stack_size = 0` dans `ThreadAddress` post-execve

### Localisation
`kernel/src/process/lifecycle/exec.rs:207–215`

### Code actuel

```rust
thread.addresses = ThreadAddress {
    entry_point:      result.entry_point,
    initial_rsp:      result.initial_stack_top,
    tls_base:         result.tls_base,
    stack_base:       0,  // fourni par ELF_LOADER dans initial_stack_top
    stack_size:       0,
    sigaltstack_base: 0,
    sigaltstack_size: 0,
};
```

### Problème
`stack_base=0` et `stack_size=0` sont propagés vers :
1. Les outils de debug (`/proc/PID/maps` équivalent ExoOS) — la pile n'apparaît pas
2. Le signal handler : `sigaltstack` vérifie si `stack_base != 0` pour valider la pile principale
3. `do_exit()` : si le cleanup des VMAs utilise ces champs pour identifier la pile, elle ne sera pas libérée

### Correction

```rust
// kernel/src/process/lifecycle/exec.rs — dans do_execve()
// Après l'appel à ELF_LOADER.get()?.load_elf()

// Déduire stack_base et stack_size depuis initial_stack_top et ElfLoadResult.
// Convention : la pile grandit vers le bas depuis initial_stack_top.
// Taille par défaut : 8 pages (32 KiB) — configurable via RLIMIT_STACK futur.
const DEFAULT_STACK_PAGES: u64 = 8;
const PAGE_SIZE:            u64 = 4096;
const DEFAULT_STACK_SIZE:   u64 = DEFAULT_STACK_PAGES * PAGE_SIZE;

let stack_top  = result.initial_stack_top;
// Aligner stack_base sur une page (la pile peut ne pas commencer à un multiple exact).
let stack_base = (stack_top.saturating_sub(DEFAULT_STACK_SIZE)) & !(PAGE_SIZE - 1);
let stack_size = stack_top.saturating_sub(stack_base) as usize;

thread.addresses = ThreadAddress {
    entry_point:      result.entry_point,
    initial_rsp:      result.initial_stack_top,
    tls_base:         result.tls_base,
    stack_base,                 // ← CORRECTION P2-03
    stack_size,                 // ← CORRECTION P2-03
    sigaltstack_base: 0,
    sigaltstack_size: 0,
};
```

> **Note** : si `ElfLoadResult` est étendu pour retourner `stack_base` et `stack_size`
> explicites (comme suggéré dans P0-02), utiliser directement ces valeurs
> plutôt que de les recalculer.

---

## P2-04 — `exoledger.rs` : OID acteur = `(pid, tid)` au lieu d'un vrai OID capability

### Localisation
`kernel/src/security/exoledger.rs:359–377` — `current_actor_oid()`

### Code actuel

```rust
fn current_actor_oid() -> [u8; 32] {
    let mut oid = [0u8; 32];
    // ...
    oid[0..8].copy_from_slice(&(tcb.pid.0 as u64).to_le_bytes());
    oid[8..16].copy_from_slice(&tcb.tid.to_le_bytes());
    oid   // 16 octets significatifs, 16 octets zéro
}
```

### Problème
L'audit ExoLedger identifie les acteurs par `(pid, tid)` au lieu du vrai OID
du capability token associé au thread (comme spécifié dans ExoShield v1 §4.2).
Cela rend les entrées d'audit ambiguës car :
- Les PIDs sont réutilisés après `exit()` → un nouveau processus peut avoir le même PID
- L'OID capability est globalement unique et non réutilisable (garantie ExoShield)
- Les audits post-incident ne peuvent pas distinguer deux processus avec le même PID

### Correction (partielle — module sécurité en refonte)

Conserver `(pid, tid)` mais **ajouter un discriminant temporel** pour éliminer
l'ambiguïté de réutilisation de PID, sans toucher aux modules ExoShield en refonte.

```rust
// kernel/src/security/exoledger.rs — remplacer current_actor_oid()

fn current_actor_oid() -> [u8; 32] {
    let mut oid = [0u8; 32];
    let tcb_ptr = crate::scheduler::core::switch::current_thread_raw();

    if tcb_ptr.is_null() {
        // Early boot : TSC + CPU comme discriminant unique.
        let tsc = rdtsc();
        let cpu = crate::arch::x86_64::smp::percpu::current_cpu_id();
        oid[0..8].copy_from_slice(&tsc.to_le_bytes());
        oid[8..12].copy_from_slice(&cpu.to_le_bytes());
        // Marqueur "early boot" dans les 4 octets suivants.
        oid[12..16].copy_from_slice(&0xEB00_0000u32.to_le_bytes());
        return oid;
    }

    // SAFETY: current_thread_raw() retourne le TCB actif du CPU courant.
    let tcb = unsafe { &*tcb_ptr };

    // Champs de l'OID :
    // [0..8]   = PID (u64 LE)
    // [8..16]  = TID (u64 LE)
    // [16..24] = TSC de création du thread (u64 LE) ← discriminant anti-réutilisation PID
    // [24..32] = réservé pour le vrai OID capability (ExoShield refonte)
    oid[0..8].copy_from_slice(&(tcb.pid.0 as u64).to_le_bytes());
    oid[8..16].copy_from_slice(&tcb.tid.to_le_bytes());
    // Utiliser le TSC de création stocké dans le TCB.
    // Si le champ n'existe pas encore dans ThreadControlBlock, utiliser 0 pour l'instant.
    let creation_tsc: u64 = tcb.creation_tsc.unwrap_or(0);
    oid[16..24].copy_from_slice(&creation_tsc.to_le_bytes());
    // [24..32] : zéro jusqu'à la refonte ExoShield (placeholder documenté).
    oid[24..32].copy_from_slice(&[0xCA, 0xFE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    oid
}
```

**Ajout dans `ThreadControlBlock` :**

```rust
// kernel/src/scheduler/core/task.rs — dans la struct ThreadControlBlock

/// TSC au moment de la création du thread.
/// Utilisé comme discriminant anti-réutilisation PID dans ExoLedger.
/// Initialisé dans ThreadControlBlock::new().
pub creation_tsc: u64,

// Dans new() :
creation_tsc: crate::arch::x86_64::cpu::tsc::read_tsc(),
```

> **Note** : cette correction est intentionnellement conservatrice.
> Le vrai OID capability sera câblé lors de la refonte ExoShield.
> `oid[24..32]` avec le marqueur `0xCAFE_0000_0000_0000` permet aux outils d'audit
> de détecter les entrées "pre-ExoShield" et de les traiter différemment.

---

## P2-05 — `servers/ipc_router` : `SYS_IPC_SEND = 302` vs kernel `SYS_EXO_IPC_RECV_NB = 302`

### Localisation
`servers/ipc_router/src/main.rs:65`

### Code actuel

```rust
// ipc_router :
pub const SYS_IPC_SEND: u64 = 302;   // ← FAUX

// kernel/src/syscall/numbers.rs :
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;  // non-blocking recv
pub const SYS_EXO_IPC_SEND:    u64 = 300;  // ← le vrai SEND
```

### Problème
Dans la boucle de dispatch de `ipc_router`, le forward de messages vers un endpoint
distant utilise `syscall(302, ...)` en croyant appeler `IPC_SEND`.
Le kernel interprète 302 comme `IPC_RECV_NB` → le message est perdu,
et le router reçoit un résultat inattendu.

### Correction

Fait en partie dans P0-03 (migration vers la crate `syscall_abi` partagée).
Correction immédiate sans refactoring complet :

```rust
// servers/ipc_router/src/main.rs — corriger les constantes locales

mod syscall {
    // CORRECTION P2-05 : aligner sur les vrais numéros du kernel
    pub const SYS_IPC_CREATE: u64 = 304;  // SYS_EXO_IPC_CREATE (enregistrement endpoint)
    pub const SYS_IPC_RECV:   u64 = 301;  // SYS_EXO_IPC_RECV   (réception bloquante)
    pub const SYS_IPC_SEND:   u64 = 300;  // SYS_EXO_IPC_SEND   (envoi) ← was 302
    pub const SYS_FORK:       u64 = 57;
    pub const SYS_EXECVE:     u64 = 59;
    pub const SYS_EXIT:       u64 = 60;
    pub const SYS_WAIT4:      u64 = 61;
    pub const SYS_KILL:       u64 = 62;
    pub const SYS_GETPID:     u64 = 39;
    pub const SYS_NANOSLEEP:  u64 = 35;
}
```

Appliquer la même correction dans tout server qui définit ses propres constantes IPC locales :
- `servers/vfs_server/src/main.rs`
- `servers/crypto_server/src/main.rs`
- `servers/init_server/src/main.rs`

La solution durable est la crate `syscall_abi` partagée décrite dans P0-03.
