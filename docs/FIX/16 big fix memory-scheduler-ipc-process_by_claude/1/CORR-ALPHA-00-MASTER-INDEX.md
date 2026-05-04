# ExoOS — Audit Profond : Modules Memory / Scheduler / IPC / Process
## Index Maître des Correctifs

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Scope :** `kernel/src/memory/`, `kernel/src/scheduler/`, `kernel/src/ipc/`, `kernel/src/process/` + docs associées  
> **Sources :** `docs/recast/ExoOS_Architecture_v7.md`, `docs/kernel/memory/MEMORY_COMPLETE.md`, `docs/Exo-OS-TLA+/Memory.tla`, `docs/Exo-OS-TLA+/ContextSwitch.tla`, code source Rust  

---

## Méthodologie

Analyse croisée de quatre couches de vérité :
1. Spécification architecture canonique (`ExoOS_Architecture_v7.md`)
2. Documentation modules (`docs/kernel/*/`)
3. Spécifications formelles TLA+ (modules `Memory.tla`, `ContextSwitch.tla`)
4. Code source Rust (`kernel/src/`)

Toute divergence entre ces couches constitue un défaut documenté ci-dessous.

---

## Classification des défauts

| Classe | Signification | Impact |
|--------|--------------|--------|
| **GRV** | Guaranteed crash / UB / violation mémoire | 🔴 Bloquant |
| **SIL** | Silent wrong behavior / doc error critique | 🟠 Majeur |
| **DOC** | Incohérence documentation pure | 🟡 Mineur |

---

## Tableau des corrections

| ID | Module | Classe | Fichier | Résumé |
|----|--------|--------|---------|--------|
| [CORR-ALPHA-01](CORR-ALPHA-01-SCHED-GRV-assert-reversed.md) | Scheduler | **GRV** | `scheduler/core/switch.rs` | `debug_assert` inversé dans `block_current_thread()` |
| [CORR-ALPHA-02](CORR-ALPHA-02-SCHED-SIL-task-layout-comment.md) | Scheduler | **SIL** | `scheduler/core/task.rs` | Commentaire layout TCB: `_pad1` à [92] au lieu de `pid` |
| [CORR-ALPHA-03](CORR-ALPHA-03-SCHED-SIL-context-switch-docstring.md) | Scheduler | **SIL** | `scheduler/core/switch.rs` | Docstring `context_switch()` : étape 9 absente + CET non documenté |
| [CORR-ALPHA-04](CORR-ALPHA-04-IPC-SIL-spsc-comment-inverted.md) | IPC | **SIL** | `ipc/ring/spsc.rs` | Commentaire algorithme SPSC inversé (head↔tail) |
| [CORR-ALPHA-05](CORR-ALPHA-05-MEM-SIL-lock-order-table.md) | Memory | **SIL** | `docs/kernel/memory/MEMORY_COMPLETE.md` | Table lock order inversée vs Architecture v7 |
| [CORR-ALPHA-06](CORR-ALPHA-06-ARCH-DOC-tcb-cold-reserve.md) | Arch Doc | **DOC** | `docs/recast/ExoOS_Architecture_v7.md §3.2` | Tableau TCB incomplet : tous les sous-champs `_cold_reserve` manquants |
| [CORR-ALPHA-07](CORR-ALPHA-07-PROCESS-GRV-vfork-mut-alias.md) | Process | **GRV** | `process/lifecycle/fork.rs` | Cast `&TCB → *mut TCB` non-sain dans `wait_for_vfork_completion()` |
| [CORR-ALPHA-08](CORR-ALPHA-08-PROCESS-SIL-signal-range.md) | Process | **SIL** | `process/signal/delivery.rs` | Plage RT signals : borne supérieure 63 au lieu de 64, non documentée |
| [CORR-ALPHA-09](CORR-ALPHA-09-IPC-SIL-ipc-init-missing-reexport.md) | IPC | **SIL** | `ipc/mod.rs` | `ipc_init()` absent des re-exports publics du module racine |
| [CORR-ALPHA-10](CORR-ALPHA-10-MEM-DOC-arch-memory-layout-gap.md) | Memory | **DOC** | `docs/kernel/memory/MEMORY_COMPLETE.md §6.1` | Omission de la déclaration `#[path = "virtual/mod.rs"] pub mod virt` |

---

## Bilan par module

| Module | GRV | SIL | DOC | Total |
|--------|-----|-----|-----|-------|
| Memory | 0 | 1 | 2 | 3 |
| Scheduler | 1 | 2 | 0 | 3 |
| IPC | 0 | 2 | 0 | 2 |
| Process | 1 | 1 | 0 | 2 |
| **TOTAL** | **2** | **6** | **2** | **10** |

---

## Priorité d'application

**Phase 0 — Immédiat (bloquant tests SMP) :**
- CORR-ALPHA-01 : assert inversé → tests scheduler block/wake garantis cassés
- CORR-ALPHA-07 : alias UB → définitement reproductible sous Miri / ASAN

**Phase 1 — Haute priorité (silent wrong behavior) :**
- CORR-ALPHA-02 : layout comment erroné → confusion développeur sur TCB
- CORR-ALPHA-03 : docstring switch incomplète → séquence CET non documentée
- CORR-ALPHA-04 : algorithme SPSC inversé → implémentation consommateur future brisée
- CORR-ALPHA-05 : lock order inversé → risque de deadlock par mauvaise lecture
- CORR-ALPHA-08 : signal range non documentée → comportement inattendu RT signals

**Phase 2 — Documentation (complétude) :**
- CORR-ALPHA-06 : TCB _cold_reserve non documenté → opacité pour audit futur
- CORR-ALPHA-09 : ipc_init non ré-exporté → API discovery difficile
- CORR-ALPHA-10 : virt module path → documentation incomplète

---

*— claude-alpha, audit ExoOS 2026-05-04*
