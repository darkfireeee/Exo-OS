# Exo-OS — Architecture Context Switch & Kthreads

## Vue d'ensemble

Le context switch Exo-OS utilise **deux conventions** :
1. `context_switch_asm` — switch régulier entre deux threads déjà exécutés
2. `kthread_trampoline` — premier démarrage d'un kthread nouvellement créé

---

## Fichiers concernés

| Fichier | Rôle |
|---------|------|
| `kernel/src/scheduler/asm/switch_asm.s` | Code ASM : `context_switch_asm`, `switch_to_new_thread`, `kthread_trampoline` |
| `kernel/src/scheduler/core/switch.rs` | Wrapper Rust : `context_switch()`, `schedule_yield()`, `block_current_thread()` |
| `kernel/src/process/lifecycle/create.rs` | `create_kthread()` — création + setup frame stack |
| `kernel/src/process/core/tcb.rs` | `KernelStack`, `ProcessThread`, `KernelStack::alloc()` |

---

## context_switch_asm — Convention de stack sauvegardée

À chaque context switch, `context_switch_asm` sauvegarde le contexte du thread *sortant* sur son stack kernel dans cet ordre (du bas vers le haut) :

```
stack_top ─────────────────────────────────────────────── (haut, adresses hautes)
   ...                (contenu d'exécution du thread)
+0  MXCSR (u32 dans u64)    ← stmxcsr / ldmxcsr
+8  x87 FCW (u16 dans u64)  ← fstcw / fldcw
+16 rbx
+24 rbp
+32 r12
+40 r13
+48 r14
+56 r15
+64 adresse de retour (rip) ← poussée par call context_switch_asm
   ────────────────────────── ← kernel_rsp pointe ici lors de la sauvegarde
```

Lors de la **restauration**, `context_switch_asm` charge `new_kernel_rsp`, lit ce frame, et `ret` reprend l'exécution du thread entrant là où il s'était arrêté.

---

## Création d'un kthread — Setup du frame initial

La fonction `create_kthread()` doit préparer un frame **artificiel** qui simule un frame `context_switch_asm` sauvegardé, de façon à ce que le premier context switch vers ce kthread fonctionne correctement.

### Layout du frame kthread (72 bytes = 9 × u64)

```
kernel_rsp ───────────────────────────────────── ← TCB::kernel_rsp pointe ici
[+0 ] MXCSR     = 0x1F80   (round-to-nearest, exceptions masquées)
[+8 ] FCW       = 0x037F   (précision étendue 64-bit, exceptions masquées)
[+16] rbx       = 0
[+24] rbp       = 0
[+32] r12       = params.entry   ← fn(usize)->! du kthread
[+40] r13       = params.arg     ← argument usize
[+48] r14       = 0
[+56] r15       = 0
[+64] ret addr  = kthread_trampoline
```

### Flux d'exécution au premier switch

```
context_switch_asm(prev, kernel_rsp_kthread, ...) :
  1. sauvegarde prev
  2. charge RSP = kernel_rsp_kthread
  3. ldmxcsr [rsp+0]   → restaure MXCSR (0x1F80)
  4. fldcw   [rsp+8]   → restaure FCW (0x037F)
  5. addq $16, %rsp
  6. popq %rbx (0), %rbp (0)
  7. popq %r12 = entry_fn    ← la fonction du kthread
  8. popq %r13 = arg
  9. popq %r14 (0), %r15 (0)
  10. ret → rip = kthread_trampoline

kthread_trampoline :
  movq %r13, %rdi     → arg dans rdi (1er paramètre SystemV)
  jmpq *%r12          → saute à entry_fn(arg)

entry_fn(arg) → !     → le kthread s'exécute, ne retourne jamais
```

---

## Erreur à éviter : frame stack trop court

```rust
// ❌ INCORRECT — frame de 16 bytes incompatible avec context_switch_asm
let rsp_ptr = (stack_top - 16) as *mut u64;
*rsp_ptr.add(0) = entry as u64;  // lu comme MXCSR → valeur invalide
*rsp_ptr.add(1) = arg as u64;    // lu comme FCW → valeur invalide
// rbx, rbp, r12..r15 lus HORS LIMITES du stack → corruption mémoire
// ret → RIP = garbage → triple fault
```

---

## MXCSR et x87 FCW par défaut

| Registre | Valeur | Signification |
|----------|--------|---------------|
| MXCSR    | `0x1F80` | Toutes les exceptions SSE masquées, arrondi au plus proche |
| x87 FCW  | `0x037F` | Toutes les exceptions x87 masquées, précision étendue (64-bit), arrondi au plus proche |

Ces valeurs sont les valeurs post-RESET standard de l'architecture x86_64.

---

## init_reaper() et process::init()

`process::init()` (appelé en Phase 4 de `kernel_init()`) démarre le kthread reaper :

```
process::init()
  └─ pid::init()                    — réserve PID 0 + PID 1
  └─ registry::init(32768)          — alloue table PCB
  └─ reap::init_reaper()            — crée kthread "reaper" via create_kthread()
  └─ wakeup::register_with_dma()   — enregistre handler DMA
  └─ cgroup::init()                 — initialise cgroup racine
```

Le kthread reaper (`reaper_loop`) tourne en boucle, drainant `REAPER_QUEUE` (SPSC lock-free 512 entrées). Il ne s'exécute que quand le scheduler effectue un context switch vers lui (nécessite le timer APIC activé — Phase future).
