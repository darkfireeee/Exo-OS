
# Exo-OS — Rapport d'audit v0.2.0 Stabilisation
**Auteur :** Claude Iota  
**Date :** 2026-05-14  
**Base :** kernel.zip — état post v0.1.0 "Elder and Bobby"  
**Objectif :** Identifier toutes les incohérences à corriger avant de passer à Wayland et à l'installeur

---

## Méthode

Lecture systématique des fichiers suivants :
`src/main.rs`, `src/lib.rs`, `src/syscall/dispatch.rs`, `src/syscall/table.rs`,
`src/syscall/net_bridge.rs`, `src/syscall/fs_bridge.rs`, `src/syscall/compat/posix.rs`,
`src/syscall/handlers/{process,signal,memory}.rs`, `src/process/lifecycle/{fork,exec,reap}.rs`,
`src/process/core/{pcb,tcb,registry}.rs`, `src/scheduler/core/{switch,runqueue}.rs`,
`src/exophoenix/forge.rs`, `src/memory/virtual/mmap.rs`, `src/memory/cow/tracker.rs`,
`servers/init_server/src/{main,service_table,boot_sequence}.rs`,
`servers/exosh/src/main.rs`, `servers/network_server/EXONET_V4_AUDIT.md`

---

## Résumé exécutif

| Priorité | Nombre | Description courte |
|---|---|---|
| P0 — Critique | 3 | Sécurité mémoire, cohérence de cycle de vie thread |
| P1 — Majeure | 5 | Fonctionnalités incomplètes bloquant la stabilité |
| P2 — Mineure | 5 | Code mort trompeur, incohérences POSIX, risques silencieux |

---

## P0 — Critique

### INC-01 : `net_sendmsg` / `net_recvmsg` transmettent un pointeur userspace brut au network_server

**Fichier :** `kernel/src/syscall/net_bridge.rs`, lignes 374–394

```rust
pub fn net_sendmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result {
    // ...
    let reply = dispatch(NET_OP_SENDMSG, fd as u32, msg_ptr, 0, flags, 0)?;
    Ok(reply.status)
}
```

`msg_ptr` est l'adresse d'une `struct msghdr` userspace. Il est passé directement dans le champ `arg1` du `NetMsg` IPC et envoyé au `network_server` **sans aucune copie kernel préalable**.

L'architecture V4 (confirmée dans `EXONET_V4_AUDIT.md`) stipule : _"userspace pointers are copied or decoded in-kernel before IPC"_. Cette règle est respectée pour `sendto`/`recvfrom` (qui copient la `sockaddr` via `copy_from_user`) mais violée pour `sendmsg`/`recvmsg`.

Le `network_server` (Ring 1) ne peut pas accéder à la mémoire utilisateur du processus appelant — il lirait une adresse invalide depuis son propre espace d'adressage, produisant soit un page fault silencieux, soit une lecture de données arbitraires.

**Correction attendue :** copier la `struct msghdr` et les `iov_base` associés en kernel avant de construire le `NetMsg`, ou passer les données utiles en payload inline dans les 40 octets `NetReply.payload`.

---

### INC-02 : `sys_rt_sigreturn` retourne `ENOSYS` sans explication dans `handlers/signal.rs`

**Fichier :** `kernel/src/syscall/handlers/signal.rs`, lignes 217–225

```rust
pub fn sys_rt_sigreturn(_a1: u64, ...) -> i64 {
    // Délègue → process::signal::handler::restore_signal_frame()
    // La vérification magic est faite dans restore_signal_frame() AVANT toute restauration.
    ENOSYS
}
```

Ce handler **n'est jamais appelé** : `dispatch.rs` intercepte `SYS_RT_SIGRETURN` (syscall 15) avant d'atteindre la table de dispatch (cf. `dispatch.rs` lignes 164–171, `handle_sigreturn_inplace`). Mais le corps du handler retourne `ENOSYS` avec un commentaire qui prétend déléguer vers `restore_signal_frame` — cette délégation n'existe pas.

Le danger est double : si le chemin spécial dans `dispatch.rs` est un jour supprimé ou réorganisé, `sys_rt_sigreturn` renverra silencieusement `ENOSYS` au lieu de restaurer les registres, corrompant l'état du thread sans panic. De plus, le commentaire trompeur peut induire un développeur à penser que `restore_signal_frame` est appelé depuis ce handler.

**Correction attendue :** remplacer le corps par `unreachable!("rt_sigreturn est géré inline dans dispatch.rs:handle_sigreturn_inplace")` ou au minimum ajouter un commentaire `// NOTE: ce handler n'est jamais appelé`.

---

### INC-03 : `sys_exit_group` ne termine pas les threads frères

**Fichier :** `kernel/src/syscall/handlers/process.rs`, lignes 122–127

```rust
pub fn sys_exit_group(status: u64, ...) -> i64 {
    // Délègue vers sys_exit pour l'instant.
    // Itération sur les threads frères require process/ pleinement intégré.
    sys_exit(status, 0, 0, 0, 0, 0)
}
```

`exit_group` est le syscall que la libc (musl-exo) émet lorsqu'un processus multithreadé se termine (`_Exit`, `exit`, ou retour de `main`). Son sémantique POSIX est de terminer **tous les threads du groupe de threads** avant de sortir.

L'implémentation actuelle termine seulement le thread appelant via `sys_exit`. Les threads frères continuent à s'exécuter en mode zombie de fait : ils n'ont plus de `mm` valide si le parent libère son espace mémoire, produisant des page faults ou des corruptions silencieuses en SMP.

Ce bug sera déclenché dès que `exosh` ou un serveur Ring1 multithreadé appelle `exit()` — ce qui se produira lors de l'intégration Wayland ou de l'installeur.

**Correction attendue :** implémenter l'itération sur `pcb.thread_list` et envoyer `SIGKILL` à tous les TCB frères avant `do_exit()`, ou utiliser `process::lifecycle::exit::do_exit_group()` une fois implémenté.

---

## P1 — Majeure

### INC-04 : `top` / `ps` utilisent une table PID→nom hardcodée, aucun syscall de liste de processus n'existe

**Fichier :** `servers/exosh/src/main.rs`, lignes 719–738 et 2088–2105

```rust
fn known_process_name(pid: u32, self_pid: u32) -> &'static [u8] {
    match pid {
        1 => b"init_server",
        2 => b"ipc_router",
        // ...
        13 => b"exosh",
        _ if pid == self_pid => b"exosh",
        _ => b"user_process",
    }
}
```

`cmd_top()` scanne les PIDs 1..=64, injecte des noms depuis cette table statique, et affiche tous les PID répondant à `kill(pid, 0) == 0` comme "running".

Deux incohérences structurelles :

1. **Aucun syscall `SYS_EXO_PROC_LIST`** n'est défini dans `numbers.rs`. Le `PROCESS_REGISTRY` noyau expose bien `for_each()` mais il n'y a pas de pont syscall vers l'espace utilisateur.
2. La borne de scan est **64 arbitrairement**, sans lien avec `MAX_PROCESSES` du kernel.

Dès qu'un processus utilisateur sera lancé (installeur, démon réseau), son nom sera affiché `user_process` et son PID réel peut dépasser 64.

**Correction attendue (deux étapes) :**
- Ajouter `SYS_EXO_PROC_LIST` (ex. numéro 335) dans `numbers.rs` + handler dans `table.rs` qui remplit un buffer `[{pid: u32, name: [u8;32]}; N]` depuis `PROCESS_REGISTRY.for_each()`.
- Réécrire `cmd_top()` pour appeler ce syscall.

---

### INC-05 : `sys_umask` est un stub sans stockage — le PCB n'a pas de champ `umask`

**Fichier :** `kernel/src/syscall/compat/posix.rs`, lignes 426–432

```rust
pub fn sys_umask(mask: u64, ...) -> i64 {
    // Note Tâche-5 : ajouter `umask: AtomicU32` dans PCB et stocker la valeur réelle.
    (mask & 0o777) as i64
}
```

Le code note lui-même le problème en commentaire. `sys_umask` retourne le masque demandé tronqué à 9 bits mais **ne le stocke nulle part**. Le PCB (`src/process/core/pcb.rs`) ne contient pas de champ `umask`.

Conséquence : toutes les créations de fichiers via ExoFS utilisent implicitement un umask de 0 (aucun bit n'est masqué), ce qui produit des permissions 0777 pour les fichiers et 0777 pour les répertoires créés par `mkdir` — comportement non conforme et potentiellement un vecteur de sécurité quand plusieurs processus sont présents.

**Correction attendue :**
- Ajouter `umask: AtomicU32` dans `ProcessControlBlock`.
- Initialiser à `0o022` à la création du processus.
- `sys_umask` : lire l'ancienne valeur, stocker la nouvelle, retourner l'ancienne (sémantique POSIX).
- Propager l'umask dans `fs_bridge::fs_open` / `fs_mkdir` lors du calcul du mode effectif.

---

### INC-06 : `ExoPhoenix forge.rs` — `reload_driver_binary_from_exofs` contient un TODO bloquant

**Fichier :** `kernel/src/exophoenix/forge.rs`, ligne 572

```rust
// TODO ExoPhoenix Phase suivante: mapper le binaire Ring1 + signaler redémarrage driver.
Ok(())
```

La fonction `reload_driver_binary_from_exofs` localise bien le blob du driver dans le cache ExoFS (`BLOB_CACHE.get`), mais **ne fait rien** ensuite — ni mapping en Ring 1, ni signal de redémarrage. Elle retourne `Ok(())` immédiatement.

`reset_all_ring1_drivers()` appelle cette fonction pour chaque device PCI après une séquence FLR + drain + IOTLB flush. Si `reload_driver_binary_from_exofs` ne recharge pas le binaire, la séquence de récupération ExoPhoenix est incomplète : les drivers sont réinitialisés au niveau hardware mais leur code Ring 1 n'est pas remis en état.

Ce TODO sera déclenché dès la première activation du chemin de récupération (crash driver, hotplug, test de robustesse réseau).

**Correction attendue :** implémenter le mapping du blob ELF dans l'espace Ring 1 du driver via `create_init_process_from_elf` ou équivalent, et notifier le supervisor Ring 1 du redémarrage.

---

### INC-07 : `sys_times` retourne des millisecondes au lieu de ticks HZ=100

**Fichier :** `kernel/src/syscall/compat/posix.rs`, lignes 489–493

```rust
// Ticks depuis le boot (ms comme approximation d'un tick HZ=1000)
let ticks = crate::scheduler::timer::clock::monotonic_ns() / 1_000_000;
ticks as i64
```

`times(2)` doit retourner une valeur en **ticks système** (CLK_TCK, généralement 100 Hz = 1/100 de seconde). L'implémentation divise par 1 000 000 pour obtenir des millisecondes, puis retourne ce résultat comme valeur de ticks.

Un processus qui boot depuis 10 secondes recevra la valeur `10 000` (10 000 ms), alors que POSIX en attend `1 000` (10 s × 100 ticks/s). La valeur retournée est **10× trop grande**, ce qui cassera tout programme qui utilise `times()` pour mesurer des durées relatives (make, shell time builtin de musl, profilers).

**Correction attendue :**
```rust
// CLK_TCK = 100 → diviser par 10_000_000
let ticks = crate::scheduler::timer::clock::monotonic_ns() / 10_000_000;
ticks as i64
```
Et `Tms.utime`/`stime` devront être nourris depuis les compteurs de temps CPU par thread (non implémentés — retourner 0 est correct pour l'instant, mais à documenter).

---

## P2 — Mineure

### INC-08 : Code mort trompeur dans `compat/posix.rs` — `getdents64`, `readlink`, `readlinkat` retournent ENOSYS mais sont câblés dans `table.rs`

**Fichier :** `kernel/src/syscall/compat/posix.rs`, lignes 434–465

```rust
pub fn sys_getdents64(...) -> i64 { ENOSYS }
pub fn sys_readlink(...)  -> i64 { ENOSYS }
pub fn sys_readlinkat(...) -> i64 { ENOSYS }
```

Ces trois handlers dans `compat/posix.rs` ne sont pas appelés : `table.rs` route `SYS_GETDENTS64`, `SYS_READLINK`, `SYS_READLINKAT` vers des implémentations complètes (lignes 3917–3920 de `table.rs`) via `fs_bridge`. Les stubs `compat/posix.rs` sont du code mort.

Le risque est qu'un refactoring inverse les routes et active accidentellement les stubs ENOSYS.

**Correction attendue :** supprimer les trois fonctions de `compat/posix.rs`, ou les remplacer par `unreachable!()`.

---

### INC-09 : `SYS_GETGROUPS`, `SYS_SETGROUPS`, `SYS_CAPGET`, `SYS_CAPSET` définis dans `numbers.rs` mais non câblés dans `table.rs`

**Fichier :** `kernel/src/syscall/numbers.rs`, lignes 160–171 (numéros 115, 116, 125, 126)

Ces quatre syscalls ont leurs constantes de numérotation mais ne sont associés à aucun handler dans `table.rs`. Ils tombent sur le stub générique ENOSYS sans compteur ni trace spécifique.

Le module `security/capability/` est complet et `pcb.creds` stocke les credentials. Il serait possible de brancher des implémentations minimales. En l'état, tout programme qui appelle `capget()` (typiquement `ping`, `sudo`, `su`) reçoit ENOSYS sans diagnostic utile.

**Correction attendue :** soit câbler des handlers minimaux (ex. `sys_capget` retourne un ensemble de capabilities vide, `sys_getgroups` retourne 0 groupe), soit documenter explicitement ces syscalls comme hors-scope v0.2.0 avec un commentaire dans `table.rs`.

---

### INC-10 : `sys_clone` ignore les flags POSIX `CLONE_VM`, `CLONE_FS`, `CLONE_FILES`, `CLONE_SIGHAND`, `CLONE_THREAD`

**Fichier :** `kernel/src/syscall/table.rs`, lignes 2074–2131

```rust
let detached = (flags & 0x0040_0000) != 0; // CLONE_DETACHED
// Aucun autre flag n'est inspecté
```

Seul `CLONE_DETACHED` est lu. Les flags standard `CLONE_VM` (0x100), `CLONE_FS` (0x200), `CLONE_FILES` (0x400), `CLONE_SIGHAND` (0x800), `CLONE_THREAD` (0x10000) sont silencieusement ignorés.

Musl émet `clone(CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|CLONE_SETTLS, ...)` pour `pthread_create`. Sans vérification de ces flags, le kernel crée un thread qui **ne partage pas** l'espace d'adressage, les file descriptors, ni les handlers de signaux avec le parent — sémantique d'un `fork`, pas d'un thread. Cela produira des corruptions dès qu'une application multithreadée (serveur réseau, démon d'installation) sera lancée.

**Correction attendue :** au minimum valider `CLONE_VM | CLONE_SIGHAND | CLONE_THREAD` ensemble et retourner `EINVAL` si la combinaison est incohérente, ou implémenter le partage d'espace d'adressage pour les threads (passer `child_cr3 = parent_cr3` au lieu de cloner).

---

### INC-11 : `SERVICES` array dans `init_server/main.rs` — `exosh` avant `exo_shield` en dépit de la dépendance

**Fichier :** `servers/init_server/src/main.rs`, lignes 91–103

```rust
static SERVICES: [...] = [
    // ...
    Service::new("exosh",      ...),   // index 10 — dépend de "exo_shield"
    Service::new("exo_shield", ...),   // index 11
];
```

`DEPS_EXOSH` inclut `"exo_shield"` comme dépendance, mais `exosh` est déclaré à l'index 10 et `exo_shield` à l'index 11. `boot_services` converge correctement (boucle `while progress`), mais nécessite une passe supplémentaire inutile.

Incohérence plus grave : si `exo_shield` (marqué `critical: false`) crashe en production, la boucle de supervision ne peut pas relancer `exosh` (ses dépendances ne sont plus satisfaites), laissant le système sans shell interactif. Si `exo_shield` était `critical: true`, le système s'arrêterait proprement. La combinaison `exosh depends exo_shield` + `exo_shield non-critique` crée un état silencieusement dégradé.

**Correction attendue :**
- Échanger `exosh` et `exo_shield` dans le tableau (mettre `exo_shield` avant `exosh`).
- Ou marquer `exo_shield` comme `critical: true` puisque `exosh` en dépend.
- Documenter la politique de dépendance non-critique explicitement.

---

## Tableau de synthèse

| ID | Priorité | Fichier principal | Impact |
|---|---|---|---|
| INC-01 | P0 | `syscall/net_bridge.rs` | Sécurité mémoire — ptr userspace brut en IPC Ring1 |
| INC-02 | P0 | `syscall/handlers/signal.rs` | Code trompeur — ENOSYS masque la véritable implémentation |
| INC-03 | P0 | `syscall/handlers/process.rs` | `exit_group` orpheline les threads frères |
| INC-04 | P1 | `servers/exosh/src/main.rs` + `syscall/numbers.rs` | `top`/`ps` hardcodés, pas de syscall proc list |
| INC-05 | P1 | `syscall/compat/posix.rs` + `process/core/pcb.rs` | `umask` non stocké, permissions fichiers toujours 0777 |
| INC-06 | P1 | `exophoenix/forge.rs` | ExoPhoenix Ring1 reload incomplet — TODO bloquant |
| INC-07 | P1 | `syscall/compat/posix.rs` | `sys_times` retourne des ms au lieu de ticks HZ=100 |
| INC-08 | P2 | `syscall/compat/posix.rs` | Code mort ENOSYS sur getdents64/readlink |
| INC-09 | P2 | `syscall/table.rs` | capget/getgroups non câblés, ENOSYS silencieux |
| INC-10 | P2 | `syscall/table.rs` | `clone` ignore CLONE_VM/THREAD/SIGHAND |
| INC-11 | P2 | `servers/init_server/src/main.rs` | exosh avant exo_shield — état dégradé silencieux |

---

## Ordre de traitement recommandé pour v0.2.0

**Bloc 1 — Sécurité / Fiabilité (INC-01, INC-03, INC-10)**  
Ces trois incohérences produiront des crashes ou des corruptions mémoire dès que le multiprocessus ou le réseau sera exercé. À corriger avant tout test d'intégration Wayland.

**Bloc 2 — Complétude fonctionnelle (INC-04, INC-05, INC-06)**  
Nécessaires pour que le système soit auto-suffisant : liste de processus, permissions fichiers correctes, récupération ExoPhoenix opérationnelle.

**Bloc 3 — Cohérence POSIX (INC-02, INC-07, INC-08, INC-09, INC-11)**  
Polissage de conformité et robustesse de l'init graph. À traiter en parallèle du Bloc 2.

---

*Rapport généré par Claude Iota — audit statique du source, sans exécution.*