# ExoOS — Index Master des Corrections
## Commit de référence : `c4239ed1`
## Date d'audit : 2026-04-20

---

## Organisation des documents

| Fichier | Contenu |
|---------|---------|
| `00_INDEX_MASTER.md` | Ce fichier — synthèse exécutive et index |
| `01_CORRECTIONS_P0_CRITIQUES.md` | Bugs bloquants — fork/execve/IPC inutilisables |
| `02_CORRECTIONS_P1_MAJEURES.md` | Bugs majeurs — comportements incorrects en production |
| `03_CORRECTIONS_P2_MINEURES.md` | Bugs mineurs — fuites, cas limites, cosmétique sécurité |
| `04_RECTIFICATION_AUDIT_PRECEDENT.md` | Points de l'audit précédent à **clore** (bugs déjà corrigés) |
| `05_ETAT_SERVERS.md` | État détaillé des servers Ring1 et gaps restants |

---

## Synthèse exécutive

### Ce qui fonctionne réellement (vérifié dans le code)
- Boot BSP→APs : séquence 18 étapes conforme, MSRs STAR/LSTAR/SFMASK initialisés sur APs
- Scheduler : CFS, context switch 6 registres, FPU XSAVE/XRSTOR, per-CPU runqueues
- Mémoire : buddy, SLUB 64B, KPTI per-CPU, HPET fixmap, swap backend enregistré
- Drivers GI-03 : syscalls 530–546 présents, IOMMU, teardown 7 étapes complet
- ExoPhoenix : `mask_all_msi_msix()` écrit réellement dans le PCI config space, `pci_function_level_reset()` réel, double SIPI OK
- ExoFS : 293 fichiers, cache, compression, audit
- Entrée syscall ASM : `syscall_entry_asm` réel dans `arch/x86_64/syscall.rs`
- IPC core : SPSC rings initialisés, `RING_MASK` correct, lock order documenté
- `do_exit()` : 7 étapes GI-03 complètes dans `driver_do_exit()`
- TLB ACK : `handoff.rs` accepte `FREEZE_ACK_DONE` ET `TLB_ACK_DONE` → pas de deadlock
- Shadow stack : `alloc_shadow_stack_pages()` appelle réellement `buddy::alloc_pages()`
- Swap provider : `register_backend_swap_provider()` appelé depuis `memory::init()`
- PID bitmap : convention `1=libre`, `fetch_and(!mask)` = marquage correct

### Bloquants actuels (rien ne tourne en userspace)
1. `fork()` et `execve()` : traits `AddressSpaceCloner` et `ElfLoader` **jamais enregistrés et jamais implémentés**
2. `ipc_router` utilise `SYS_IPC_REGISTER=300` mais le kernel a `SYS_EXO_IPC_SEND=300` → **endpoint jamais enregistrable**
3. `sys_exo_ipc_send/recv` retournent **ENOSYS** (câblage `ipc::channel` manquant)
4. `sys_read/write/open/close` retournent **ENOSYS** (fs_bridge non câblé dans table.rs)

---

## Tableau récapitulatif de tous les bugs

### P0 — Critiques

| ID | Fichier principal | Description courte | Document |
|----|-------------------|--------------------|----------|
| **P0-01** | `process/lifecycle/fork.rs` + `dispatch.rs` | `AddressSpaceCloner` jamais impl/enregistré → `fork()` = EFAULT systématique | 01 |
| **P0-02** | `process/lifecycle/exec.rs` + `dispatch.rs` | `ElfLoader` jamais impl/enregistré → `execve()` = ENOSYS systématique | 01 |
| **P0-03** | `syscall/numbers.rs` + `servers/ipc_router` | Numéros syscall IPC incohérents server↔kernel : 300=REGISTER (server) vs 300=SEND (kernel) | 01 |
| **P0-04** | `syscall/table.rs` + `syscall/fs_bridge.rs` | `sys_read/write/open/close` → ENOSYS, fs_bridge non câblé | 01 |
| **P0-05** | `syscall/table.rs` | `sys_exo_ipc_send/recv/call` → ENOSYS, câblage `ipc::channel` manquant | 01 |

### P1 — Majeures

| ID | Fichier principal | Description courte | Document |
|----|-------------------|--------------------|----------|
| **P1-01** | `process/lifecycle/fork.rs` | Fuite PML4 CoW sur `RegistryError`/`InvalidCpu` dans `do_fork()` | 02 |
| **P1-02** | `ipc/shared_memory/mapping.rs` | SHM : `virt_addr = phys_addr` stub — non fonctionnel pour userspace | 02 |
| **P1-03** | `syscall/validation.rs` | Fixup ASM `#PF` absent → page fault en handler syscall = kernel panic | 02 |
| **P1-04** | `ipc/ring/spsc.rs` | `MAX_SPSC_RINGS=256` vs `MAX_CHANNELS=65536` — canaux >255 rejetés | 02 |
| **P1-05** | `exophoenix/stage0.rs` | ExoPhoenix SIPI : pas d'INIT IPI avant les deux SIPIs (spec Intel MP §B.4) | 02 |

### P2 — Mineures

| ID | Fichier principal | Description courte | Document |
|----|-------------------|--------------------|----------|
| **P2-01** | `arch/x86_64/syscall.rs` | `syscall_cstar_noop` ne restaure pas RSP userspace avant `sysret` | 03 |
| **P2-02** | `process/lifecycle/fork.rs` | Fork fils : RFLAGS figé à `0x0202`, flags parent (AC, DF...) non hérités | 03 |
| **P2-03** | `process/lifecycle/exec.rs` | `stack_base=0, stack_size=0` dans `ThreadAddress` post-execve | 03 |
| **P2-04** | `security/exoledger.rs` | OID acteur = `(pid,tid)` codé — pas un vrai OID capability token | 03 |
| **P2-05** | `servers/ipc_router` | `SYS_IPC_SEND=302` dans ipc_router vs `SYS_EXO_IPC_RECV_NB=302` dans kernel | 03 |

---

## Ordre de traitement recommandé

```
Phase 1 (débloque le userspace) :
  P0-01 → P0-02 → P0-03 → P0-04 → P0-05

Phase 2 (stabilité production) :
  P1-01 → P1-02 → P1-03 → P1-04 → P1-05

Phase 3 (robustesse) :
  P2-01 → P2-02 → P2-03 → P2-04 → P2-05
```
