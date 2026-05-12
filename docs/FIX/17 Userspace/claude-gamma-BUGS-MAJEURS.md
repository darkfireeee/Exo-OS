# ExoOS — Bugs Majeurs — claude-gamma
## Fichier : claude-gamma-BUGS-MAJEURS.md

---

## BUG-M1 — `wait_for_ipc_ready` n'est pas une vraie barrière de readiness IPC
**Sévérité : MAJEUR — démarrage de services fragile, race conditions**  
**Fichier** : `servers/init_server/src/boot_sequence.rs`

### Description

```rust
pub unsafe fn wait_for_ipc_ready(pid: u32, timeout_ms: u64) -> bool {
    let mut waited_ms = 0u64;
    while waited_ms <= timeout_ms {
        if pid_alive(pid) {   // ← juste kill(pid, 0)
            sleep_ms(IPC_READY_SETTLE_MS); // ← attente fixe de 10ms
            return true;
        }
        sleep_ms(POLL_INTERVAL_MS);
        waited_ms += POLL_INTERVAL_MS;
    }
    false
}
```

Cette fonction retourne `true` dès que `kill(pid, 0) == 0` + 10ms. Elle ne vérifie pas que le service a enregistré son endpoint IPC. Les services suivants peuvent démarrer avant que le service précédent ait terminé son `_start()`.

### Impact

- `memory_server` peut démarrer avant que `ipc_router` ait son endpoint
- `vfs_server` peut démarrer avant que `memory_server` soit prêt
- Résultat : les appels IPC du démarrage échouent / timeout → services tués par init → relance avec backoff exponentiel → démarrage beaucoup plus long

### Correction

Ajouter un mécanisme de handshake IPC explicite : chaque service envoie un message `IPC_READY` à l'init_server quand son `_start()` est terminé. `wait_for_ipc_ready` attend ce message via IPC receive avec timeout.

---

## BUG-M2 — Scheduler server dépend de "init_server" (lui-même)
**Sévérité : MAJEUR — scheduler_server ne peut potentiellement jamais démarrer**  
**Fichier** : `servers/init_server/src/service_table.rs`

### Description

```rust
const DEPS_SCHEDULER: &[&str] = &["init_server"];
```

`init_server` IS le processus courant (PID 1). Il n'est pas dans le tableau `SERVICES[]`. La fonction `can_start` vérifie que toutes les dépendances ont un PID non-nul dans `SERVICES` :

```rust
// supervisor.rs — can_start
pub fn can_start(services: &[Service], name: &str) -> bool {
    if let Some(meta) = service_table::metadata(name) {
        for dep in meta.requires {
            if runtime_index_by_name(services, dep.as_bytes()).is_none() {
                return false; // "init_server" non trouvé dans services → false
            }
            // ...
        }
    }
    true
}
```

`runtime_index_by_name(services, b"init_server")` retourne `None` car "init_server" n'est pas dans `SERVICES[]`. Donc `can_start("scheduler_server")` retourne `false` → `scheduler_server` ne démarre jamais.

### Correction

Changer la dépendance ou la supprimer :

```rust
// Option 1 : pas de dépendance pour scheduler_server (il démarre après les critiques)
const DEPS_SCHEDULER: &[&str] = &["memory_server", "ipc_router"];

// Option 2 : traiter "init_server" comme un service toujours présent dans can_start
```

---

## BUG-M3 — Binaire exosh dans `userspace/apps/exosh` utilise `std`
**Sévérité : MAJEUR — mauvais binaire si compilé pour bare-metal**  
**Fichier** : `userspace/apps/exosh/src/main.rs`

### Description

```rust
use std::env;
use std::io::{self, Write};
use std::process::Command;  // ← libc requis
```

Ce binaire n'est PAS bare-metal. Si le pipeline de build compile ce `userspace/apps/exosh` au lieu de `servers/exosh` pour l'embedder dans l'ISO, le binary sera inutilisable (il fera des appels libc que l'OS ne supporte pas).

### Vérification nécessaire

Confirmer dans le `Makefile` et `build.rs` quel binary est embarqué pour `/bin/exosh` dans `EMBEDDED_PAYLOADS`. Le bon binaire est `servers/exosh/src/main.rs` (`#![no_std]`, `#![no_main]`, uniquement `exo_syscall_abi`).

### Correction

Si `userspace/apps/exosh` est le binaire cible, le réécrire en `no_std` comme `servers/exosh`. Sinon, clarifier dans le Makefile lequel est utilisé et supprimer l'ambiguïté.

---

## BUG-M4 — `fork_child_trampoline` : SWAPGS requis mais non vérifiable
**Sévérité : MAJEUR (potentiel) — crash ou violation de sécurité si absent**  
**Fichier** : assembleur non fourni dans le zip (référencé dans `fork.rs`)

### Description

Dans `do_fork`, le fils est configuré pour retourner via `fork_child_trampoline` + `IRETQ`. L'`IRETQ` restaure CS/SS/RIP/RSP/RFLAGS depuis la pile noyau.

**Problème** : avant l'`IRETQ`, le GS doit être swappé (kernel GS → user GS). Sans `SWAPGS`, quand le fils arrive en userspace, il lit le GS noyau au lieu du GS user. Tout accès à `%fs` (TLS) ou `%gs` sera vers des données noyau → crash ou pire (infoleak / privilege escalation).

L'assembleur de `fork_child_trampoline` n'est pas inclus dans le zip uploadé. Son absence empêche la vérification.

### Correction

Le trampoline doit suivre exactement ce schéma :

```asm
fork_child_trampoline:
    xor     eax, eax        ; retour fils = 0
    swapgs                  ; kernel GS → user GS (OBLIGATOIRE)
    iretq                   ; restaure RIP, CS, RFLAGS, RSP, SS depuis la pile
```

---

## BUG-M5 — CoW tracker : taille fixe de 4096 entrées, pas de protection OOM
**Sévérité : MAJEUR (scalabilité) — saturation silencieuse sous charge**  
**Fichier** : `kernel/src/memory/cow/tracker.rs`

### Description

```rust
pub const COW_TABLE_SIZE: usize = 4096;
```

La table CoW ne supporte que 4096 frames partagés simultanément. Si un processus avec beaucoup de pages forke (ex: un serveur avec 100 MB de RAM mappée), le tracker peut se saturer. `try_inc` retourne une erreur → `track_cow_frame` retourne `OutOfMemory` → `clone_cow` échoue → `fork()` échoue avec ENOMEM.

Sous charge réelle (plusieurs services + shell + processus fils), ce plafond peut être atteint.

### Correction

Utiliser une table dynamique (par ex. un arbre ou une hashmap allouée dynamiquement), ou augmenter `COW_TABLE_SIZE` et utiliser une structure lock-free plus robuste.

---

## BUG-M6 — `do_execve` ne remet pas à zéro `open_count` dans la FD table
**Sévérité : MOYEN — fuite de compteurs, comportement indéfini pour wait/select**  
**Fichier** : `kernel/src/process/lifecycle/exec.rs`

### Description

`close_on_exec()` ferme les FDs `O_CLOEXEC` mais ne remet pas à zéro `open_count` ni `close_count`. Ces compteurs, utilisés pour instrumenter les FD operations, peuvent devenir incohérents après exec. Les syscalls qui dépendent de `open_fd_count()` (ex: fork → `fd_limit = f.open_fd_count().max(1024)`) peuvent recevoir une valeur gonflée.

---

## BUG-M7 — Aucune gestion du signal SIGCHLD dans `init_server` pour les services qui n'ont jamais démarré
**Sévérité : MOYEN — boucle de relance inefficace**  
**Fichier** : `servers/init_server/src/main.rs`

### Description

Dans la boucle de supervision, si un service ne démarre jamais (fork() échoue → pid=0), `mark_dead()` n'est pas appelé et `restart_delay_ticks` reste à 1. La boucle retente immédiatement à chaque itération, créant une boucle serrée CPU-intensive si fork() est systématiquement en erreur (ex: ENOMEM).

### Correction

Détecter les échecs de `spawn_service` (pid=0) et appliquer le backoff exponentiel dans ce cas aussi.

---

## BUG-M8 — `handle_execve_inplace` : CR3 switché par `do_execve` mais frame non invalidée
**Sévérité : MOYEN — potentiel stale TLB entries après execve**  
**Fichier** : `kernel/src/process/lifecycle/exec.rs` + `dispatch.rs`

### Description

`do_execve` écrit le nouveau CR3 via `write_cr3(elf_result.cr3)` en milieu de l'exécution kernel. Ensuite, `handle_execve_inplace` met `frame.rcx = new_rip`, `frame.rsp = new_rsp` et retourne pour que `dispatch()` fasse un SYSRETQ.

Entre `write_cr3()` et le SYSRETQ, le kernel continue à s'exécuter avec le nouveau CR3. Si du code kernel accède des structures qui n'étaient mappées que dans l'ancien CR3 (cache de structure PCB en virtual addresses utilisateurs, par exemple), il obtiendrait des fautes.

### Évaluation

Ce bug est probablement mineur en pratique car le kernel ne devrait pas accéder à des adresses virtuelles utilisateurs après le `write_cr3`. À vérifier que `post_dispatch` (qui appelle `check_and_deliver_signals`) ne touche pas de pointeurs stale.

---

## Résumé des bugs majeurs

| ID | Fichier | Impact | Priorité |
|---|---|---|---|
| BUG-M1 | `boot_sequence.rs` | Démarrage fragile | 1 |
| BUG-M2 | `service_table.rs` | scheduler_server bloqué | 1 |
| BUG-M3 | `userspace/apps/exosh` | Mauvais binary | 2 |
| BUG-M4 | `switch_asm.s` (absent) | Crash/infoleak si SWAPGS manquant | 1 |
| BUG-M5 | `cow/tracker.rs` | Saturation CoW | 3 |
| BUG-M6 | `exec.rs` | Compteurs FD incohérents | 3 |
| BUG-M7 | `init_server/main.rs` | CPU spin sur fork échoué | 2 |
| BUG-M8 | `exec.rs` + `dispatch.rs` | TLB stale potentiel | 2 |
