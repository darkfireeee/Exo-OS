# Exo-OS — Historique des corrections de boot

Ce document retrace les bugs critiques découverts et corrigés pendant la séquence de boot du kernel.

---

## BUG-BOOT-01 — LAPIC LVT LINT0 : vecteur 0x8E non masqué (commit 40da75e)

### Symptôme
Le kernel stoppait après la phase IPC (marqueur `9` visible sur port 0xE9), sans jamais atteindre l'init FS (marqueur `!`).

```
XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]
456789          ← stoppé ici, jamais de !I\nOK\n
```

### Diagnostic (QEMU `-d int,cpu_reset`)
```
0: v=8e e=0000 i=1 cpl=0 IP=0008:0000000000180001   ← IRQ hardware livré, vecteur 0x8E
check_exception old: 0xffffffff new 0xd
1: v=0d e=0472 i=0 cpl=0 IP=...                      ← #GP : sélecteur IDT index 0x8E absent
→ triple fault → reset
```

Code d'erreur `0x0472` : `(0x0472 >> 3) = 0x8E` = entrée IDT[0x8E] marquée présente mais sans handler.

### Cause racine
Le BIOS QEMU q35/EDK2 laisse `LAPIC_LVT_LINT0` configuré en mode **Fixed delivery, vecteur 0x8E, non masqué**. La fonction `enable_xapic()` activait le LAPIC sans réinitialiser les entrées LVT héritées du BIOS. À la première interruption PIC, LINT0 la livrait via le vecteur 0x8E → aucun handler IDT → #GP → triple fault.

### Correction
**`kernel/src/arch/x86_64/apic/mod.rs`** : `init_apic_system()` appelle `local_apic::init_local_apic()` (masque tous les LVT) au lieu de `local_apic::enable_xapic()` (activation seule).

**`kernel/src/arch/x86_64/apic/x2apic.rs`** : ajout de `mask_all_lvt_x2apic()` pour le chemin x2APIC.

**Règle retenue** : toujours appeler `init_local_apic()` (pas `enable_xapic()`) pour neutraliser l'état LVT laissé par le firmware.

### Résultat
Boot complet confirmé sur QEMU TCG q35 :
```
XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]
456789!I
OK
```

---

## BUG-BOOT-02 — `create_kthread` : frame stack incompatible avec `context_switch_asm`

### Symptôme
Phase 4 (`process::init()` → `init_reaper()`) désactivée avec le commentaire :
> *TEMPORAIREMENT DÉSACTIVÉ : crash GPF f000ff53f000ff53 pendant create_kthread*

L'adresse `0xf000ff53` correspond à la zone BIOS/reset vector en mémoire basse, signature d'une corruption de pointeur de code.

### Cause racine
`create_kthread()` dans `kernel/src/process/lifecycle/create.rs` configurait le stack du nouveau kthread avec seulement 2 entrées :
```rust
// AVANT (incorrect)
let rsp_ptr = (stack_top - 16) as *mut u64;
*rsp_ptr.add(0) = params.entry as u64;   // [rsp+0]
*rsp_ptr.add(1) = params.arg as u64;     // [rsp+8]
(*thread_ptr).sched_tcb.kernel_rsp = stack_top - 16;
```

Or, `context_switch_asm` (dans `scheduler/asm/switch_asm.s`) restaure les registres dans cet ordre strict depuis `kernel_rsp` :
```asm
ldmxcsr 0(%rsp)    // [rsp+ 0] : MXCSR  ← lisait entry_fn → valeur invalide
fldcw   8(%rsp)    // [rsp+ 8] : FCW    ← lisait arg      → valeur invalide
addq $16, %rsp
popq %rbx          // [rsp+16]  ← lecture HORS STACK (passé le sommet) → corruption
popq %rbp          // [rsp+24]  ← idem → %rbp=garbage
popq %r12          // [rsp+32]  ← idem
popq %r13          // [rsp+40]
popq %r14          // [rsp+48]
popq %r15          // [rsp+56]
ret                // [rsp+64]  ← RIP = adresse BIOS (0xf000ff53 = garbage)
```

Le `ret` final sautait vers une adresse BIOS héritée de la mémoire non initialisée.
*(Aggravé par BUG-BOOT-01 : le LAPIC non masqué déclenchait ce chemin prématurément.)*

### Correction

#### 1. `kernel/src/scheduler/asm/switch_asm.s` — ajout de `kthread_trampoline`
```asm
kthread_trampoline:
    movq  %r13, %rdi    // arg (r13) → rdi (1er paramètre SystemV AMD64)
    jmpq  *%r12         // saute à entry_fn(arg) — ne retourne jamais
```

#### 2. `kernel/src/process/lifecycle/create.rs` — frame stack correct (72 bytes = 9 × u64)
```rust
// APRÈS (correct)
const FRAME: u64 = 9 * 8;
let kernel_rsp = stack_top - FRAME;
let frame = kernel_rsp as *mut u64;
*frame.add(0) = 0x0000_1F80;               // MXCSR par défaut
*frame.add(1) = 0x0000_037F;               // x87 FCW par défaut
*frame.add(2) = 0;                          // rbx
*frame.add(3) = 0;                          // rbp
*frame.add(4) = params.entry as u64;        // r12 → entry_fn (lu par trampoline)
*frame.add(5) = params.arg as u64;          // r13 → arg (lu par trampoline)
*frame.add(6) = 0;                          // r14
*frame.add(7) = 0;                          // r15
*frame.add(8) = kthread_trampoline as u64;  // adresse de retour
(*thread_ptr).sched_tcb.kernel_rsp = kernel_rsp;
```

#### 3. `kernel/src/lib.rs` — Phase 4 réactivée
`process::init()` est maintenant appelé, ce qui initialise :
- `pid::init()` — réserve PID 0 (idle) et PID 1 (init)
- `registry::init()` — alloue la table PCB (32 768 slots)
- `lifecycle::reap::init_reaper()` — enfile le kthread reaper
- `state::wakeup::register_with_dma()` — enregistre le handler DMA
- `resource::cgroup::init()` — initialise le cgroup racine

---

## Séquence de boot complète (post-corrections)

| Marqueur port 0xE9 | Phase                              | Code                            |
|--------------------|------------------------------------|---------------------------------|
| `X`                | _start ASM (64-bit confirmé)       | main.rs / boot asm              |
| `K`                | kernel_main entry                  | main.rs                         |
| `1`                | arch_boot_init démarré             | arch/x86_64/boot/early_init.rs  |
| `2..g`             | Probes arch (CPU/ACPI/APIC/HPET/PIT/TSS/SMP/GDT/IDT/LAPIC/IOREDIRECT/PML4) | arch/  |
| `Z`                | Mode 64-bit + arch init terminée   | arch/                           |
| `A`                | arch_boot_init retourné à kernel_init | lib.rs                       |
| `2`                | EmergencyPool init                 | memory/physical/frame/emergency_pool |
| `3`                | Heap allocator init (SLUB + large) | memory/heap/allocator/hybrid    |
| `4`                | time_init (HPET + TSC + ktime)     | arch/x86_64/time/               |
| `5`                | Scheduler init                     | scheduler/                      |
| `6`                | Idle thread BSP (CPU 0)            | lib.rs                          |
| `P`                | Process init + reaper kthread      | process/                        |
| `7`                | Security / capabilities            | security/capability/            |
| `8`                | Futex seed (SipHash anti-DoS)      | memory/utils/futex_table        |
| `9`                | IPC SPSC rings                     | ipc/ring/spsc/                  |
| `!`                | ExoFS init                         | fs/exofs/                       |
| `I\nOK\n`          | kernel_main idle loop              | main.rs                         |

### Calibration TSC (QEMU, etat v0.1.0)
```
[CAL:PIT-DRV hz=2614777097]
[TIME-INIT hz=2614800000]
```

Les anciens logs `[CAL:PIT-DRV-FAIL][CAL:FB3G]` correspondent a l'etat pre-v0.1.0. Le calcul PIT etait rejete car la frequence etait multipliee par erreur par 100. Voir `BUG-BOOT-03`.

---

## Vecteurs LAPIC configurés après init_local_apic()

| Entrée LVT  | Valeur configurée             | Description                       |
|-------------|-------------------------------|-----------------------------------|
| THERMAL     | `0x0001_0000` (masqué)        | Température CPU — masqué          |
| PERF        | `0x0001_0000` (masqué)        | Monitoring perf — masqué          |
| CMCI        | `0x0001_0000` (masqué)        | Corrected Machine Check — masqué  |
| LINT0       | `0x0001_0000` (masqué)        | **CRITIQUE** : était 0x8E BIOS    |
| LINT1       | `0x0000_0400` (NMI)           | NMI — non masqué (comportement x86 standard) |
| ERROR       | `0x0000_00FE` (vecteur 0xFE)  | = `VEC_IPI_PANIC`                 |
| TPR         | `0` (toutes priorités OK)     | Task Priority Register            |

---

## Prochaines étapes après v0.1.0

1. **Process list syscall** : remplacer les noms PID fixes du shell par une vraie enumeration kernel.
2. **Smoke QEMU stable** : rendre le pilotage clavier QMP moins sensible au timing hote.
3. **APIC timer / preemption** : poursuivre la preemption timer robuste sur le chemin interactif.
4. **SMP** : stabiliser le boot APs via INIT/STARTUP IPI + trampoline.
5. **ExoFS benchmarks** : utiliser `time` et `dd` pour mesurer lecture/ecriture sequentielles.

---

## BUG-BOOT-03 — Calibration PIT TSC multipliee par 100

### Symptome

Le boot userspace fonctionnait, mais le log temps restait en fallback fixe:

```text
[CAL:PIT-DRV-FAIL][CAL:FB3G hz=3000000000][TIME-INIT hz=3000000000]
```

### Cause racine

`calibrate_tsc_with_pit()` mesurait deja une fenetre reelle:

```text
seconds = PIT_CALIBRATE_COUNT / PIT_BASE_HZ
```

La formule appliquee etait pourtant:

```text
tsc_hz = tsc_delta * PIT_BASE_HZ / PIT_CALIBRATE_COUNT * 100
```

La mesure obtenue etait donc environ 100 fois trop grande. La validation rejetait la frequence comme hors plage, puis la chaine de calibration tombait sur `FB3G`.

### Correction

Formule finale:

```text
tsc_hz = tsc_delta * PIT_BASE_HZ / PIT_CALIBRATE_COUNT
```

### Resultat

Boot QEMU valide:

```text
[CAL:PIT-DRV hz=2614777097][TIME-INIT hz=2614800000]
```

Le fallback fixe `FB3G` n'apparait plus sur le chemin valide.
