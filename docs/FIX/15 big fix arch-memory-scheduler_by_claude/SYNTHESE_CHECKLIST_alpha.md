# Synthèse Finale & Checklist d'Application
## Audit Claude-Alpha — ExoOS modules arch / memory / scheduler

**Auteur** : claude-alpha  
**Date** : 2026-05-03  
**Dépôt** : HEAD cloné en direct depuis `https://github.com/darkfireeee/Exo-OS.git`

---

## Vue d'ensemble

L'audit couvre **1 400+ lignes de code source** lues directement depuis le dépôt,
croisées avec les specs TLA+ (Memory.tla, ContextSwitch.tla) et la documentation
Architecture v7 / Kernel Types v10 / ExoPhoenix Spec v6.

**10 défauts identifiés** : 4 GRV (crash garanti / corruption mémoire), 6 SIL.

---

## Ordre d'application recommandé

### Priorité absolue — GRV (à corriger avant tout test sur bare-metal)

| Ordre | ID | Fichier | Difficulté | Risque si ignoré |
|---|---|---|---|---|
| 1 | **ALPHA-01** | `arch/x86_64/idt.rs` | ⬤ trivial (1 ligne) | Triple Fault sur toute exception #DF → reset machine |
| 2 | **ALPHA-08** | `scheduler/core/runqueue.rs` | ⬤ faible (refactoring) | nr_running corrompu → load balancing aveugle SMP |
| 3 | **ALPHA-03** | `scheduler/core/task.rs` + `switch.rs` | ⬤⬤ moyen (nouveau champ TCB) | RSP0 erroné → possible stack smash IRQ |
| 4 | **ALPHA-10** | `exophoenix/forge.rs` | ⬤⬤ moyen (buffer statique) | Fuite mémoire non bornée sur cycles ExoPhoenix |

### Priorité haute — SIL (avant mise en production)

| Ordre | ID | Fichier | Difficulté | Impact si ignoré |
|---|---|---|---|---|
| 5 | **ALPHA-09** | `memory/physical/frame/emergency_pool.rs` | ⬤⬤ moyen (bitmap 4×u64) | Pool SchedNode épuisé sous charge → panic ou spin |
| 6 | **ALPHA-02** | `arch/x86_64/tss.rs` | ⬤ trivial (suppression) | 12 MiB BSS gaspillés sur 256 CPUs |
| 7 | **ALPHA-05** | `scheduler/core/task.rs` | ⬤⬤ moyen (nouvelle méthode) | API documentée non implémentable par les appelants |
| 8 | **ALPHA-07** | `scheduler/core/switch.rs` | ⬤ trivial (commentaire) | Confusion FPU → risque régression future |
| 9 | **ALPHA-04** | `docs/kernel/scheduler/SCHEDULER_CORE.md` | ⬤ trivial (doc) | Désynchronisation doc/code ThreadId |
| 10 | **ALPHA-06** | `docs/kernel/scheduler/SCHEDULER_CORE.md` | ⬤ trivial (doc) | Désynchronisation doc/code TaskState |

---

## Checklist d'application

### ALPHA-01 ✅ — IDT Double Fault

```
□ Ouvrir kernel/src/arch/x86_64/idt.rs
□ Localiser l'entrée EXC_DOUBLE_FAULT dans init_idt()
□ Changer TRAP_GATE → INTERRUPT_GATE
□ Ajouter debug_assert!(entries[8].flags == 0x8E)
□ Compiler : cargo build --target x86_64-unknown-none
□ Vérifier avec qemu -M smm=off + int3 en Ring0 → pas de Triple Fault
```

### ALPHA-08 ✅ — RunQueue nr_running

```
□ Ouvrir kernel/src/scheduler/core/runqueue.rs
□ Dans pick_next(), branche RT : remplacer le bloc par un appel à self.dequeue_highest_rt()
□ S'assurer que dequeue_highest_rt() est la seule méthode à décrémenter nr_running pour RT
□ Écrire test unitaire : enqueue 1 thread RT, pick_next, vérifier nr_running == 0
□ Tester la branche DL de la même façon
```

### ALPHA-03 ✅ — kstack_top

```
□ Lire CORRECTIF_ALPHA03_RAFFINE_KSTACK_TOP.md en entier
□ Modifier task.rs :
  □ Ajouter const KSTACK_TOP_COLD_OFFSET: usize = 32
  □ Ajouter méthodes kstack_top() et init_kstack_top()
  □ Ajouter assertions compile-time offset 176 et non-chevauchement
  □ Dans new() : appeler init_kstack_top(kernel_stack_top)
  □ Mettre à jour le commentaire de layout (ligne 176)
□ Modifier switch.rs :
  □ Remplacer next.kstack_ptr par next.kstack_top() dans update_rsp0()
  □ Remplacer next.kstack_ptr par next.kstack_top() dans set_kernel_rsp()
  □ Mettre à jour le commentaire de la séquence (étape 9)
□ Vérifier : cargo check --target x86_64-unknown-none
□ Test : voir section "Test de non-régression" dans le correctif raffiné
```

### ALPHA-10 ✅ — Forge Box::leak()

```
□ Ouvrir kernel/src/exophoenix/forge.rs
□ Supprimer l'import implicite Box::leak
□ Ajouter FORGE_IMAGE_BUF: spin::Mutex<Option<Vec<u8>>>
□ Refactoriser load_a_image_from_exofs() pour utiliser le buffer statique
□ Vérifier que le Cargo.toml kernel inclut spin (déjà présent dans le workspace)
□ Test : déclencher 3 cycles Forge consécutifs, vérifier que le heap ne croît pas
```

### ALPHA-09 ✅ — SchedNodePool 256 blocs

```
□ Ouvrir kernel/src/memory/physical/frame/emergency_pool.rs
□ Changer SCHED_POOL_SIZE de 64 à 256
□ Refactoriser SchedNodePool : remplacer AtomicU64 unique par [AtomicU64; 4]
□ Adapter alloc() : itérer sur les 4 mots du bitmap
□ Adapter free() : calculer word_idx = global_idx / 64, bit_idx = global_idx % 64
□ Adapter new_uninit() et init()
□ Ajouter assertion const : 4 * 64 == SCHED_POOL_SIZE
□ Tests existants (sched_node_pool_capacity_stress) à adapter pour 256 blocs
```

### ALPHA-02 ✅ — TSS piles mortes

```
□ Ouvrir kernel/src/arch/x86_64/tss.rs
□ Supprimer nmi_stack, ist5_stack, ist6_stack de PerCpuStacks
□ Renommer les champs restants (voir correctif P1)
□ Adapter les références dans init_tss_for_cpu()
□ Vérifier : aucune référence à nmi_stack/ist5_stack/ist6_stack dans le codebase
```

### ALPHA-05 ✅ — try_transition()

```
□ Ouvrir kernel/src/scheduler/core/task.rs
□ Ajouter impl ThreadControlBlock { try_transition(), force_transition() }
□ Vérifier que les constantes SCHED_STATE_MASK et TaskState::from_u8 sont accessibles
□ Compiler et exécuter les tests unitaires
```

### ALPHA-07 ✅ — Commentaire FFI

```
□ Ouvrir kernel/src/scheduler/core/switch.rs
□ Localiser le bloc extern "C" { fn context_switch_asm(...) }
□ Remplacer le commentaire (voir correctif P1)
```

### ALPHA-04 + ALPHA-06 ✅ — Documentation

```
□ Ouvrir docs/kernel/scheduler/SCHEDULER_CORE.md
□ Corriger ThreadId(u32) → ThreadId(u64)
□ Remplacer Blocked par Sleeping + Uninterruptible (tableau complet)
□ Mettre à jour la description de try_transition()
```

---

## Impact total après application

| Dimension | Avant | Après |
|---|---|---|
| Bugs GRV actifs | 4 | 0 |
| Bugs SIL actifs | 6 | 0 |
| BSS gaspillé (256 CPUs) | ~28 MiB | ~16 MiB (-12 MiB) |
| SchedNodePool capacité | 64 nœuds | 256 nœuds |
| `nr_running` intégrité | non garantie (double décrément possible) | garantie |
| RSP0 correctness | non (mid-stack) | oui (kstack_top) |
| Fuite Forge | oui (Box::leak non bornée) | non (buffer réutilisé) |
| `try_transition()` disponible | non | oui |
| Doc/code synchronisés | non (4 points) | oui |
| Commentaire FPU trompeur | oui | non |

---

## Fichiers produits par cet audit

```
output_servers/
├── AUDIT_ALPHA_ARCH_MEMORY_SCHEDULER.md      ← rapport principal (10 bugs)
├── CORRECTIFS_P0_GRV_alpha.md                ← patchs bugs GRV (ALPHA-01,03,08,10)
├── CORRECTIFS_P1_SIL_alpha.md                ← patchs bugs SIL (ALPHA-02,04,05,06,07,09)
├── CORRECTIF_ALPHA03_RAFFINE_KSTACK_TOP.md   ← correctif détaillé avec offsets précis
└── SYNTHESE_CHECKLIST_alpha.md               ← ce fichier
```

---

## Note sur les modules non audités dans cette session

Par manque de portée, les modules suivants n'ont **pas** été couverts dans ce cycle :

- `kernel/src/arch/x86_64/syscall.rs` — entrée SYSCALL/SYSRET
- `kernel/src/arch/x86_64/spectre/` — KPTI/retpoline (partiellement couvert via cross-ref)
- `kernel/src/scheduler/fpu/` — XSAVE/XRSTOR handlers
- `kernel/src/ipc/` — ring buffers, endpoints
- `kernel/src/exophoenix/stage0.rs` — bootstrap Kernel B

Un prochain cycle d'audit devrait couvrir `syscall.rs` (surface d'attaque Ring3→Ring0) et `fpu/` (interaction avec ALPHA-03 et le chemin Lazy FPU).

---

*— claude-alpha, audit statique sur sources directement clonées*
