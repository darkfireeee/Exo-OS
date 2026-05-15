# ExoOS — Audit Kernel v0.2.0 — Rapport de Stabilisation Complète
**Auteur :** `claude-gamma`  
**Date :** 2026-05-14  
**Base :** Codebase `kernel.zip` — build post v0.1.0 (shell fonctionnel, boot validé QEMU)  
**Objectif :** Identifier toutes les incohérences bloquantes avant la milestone v0.2.0 (stabilisation kernel complète, pré-Wayland)

---

## Résumé Exécutif

L'audit couvre l'intégralité du kernel (`kernel/src/`), les serveurs Ring 1 (`servers/`), les drivers (`drivers/`), et les logs QEMU disponibles. Le boot est fonctionnel et exosh répond. Toutefois **18 incohérences structurelles** ont été identifiées, réparties sur quatre niveaux de sévérité. Sans correction, les niveaux P0 et P1 compromettent directement la stabilité cible de v0.2.0.

| Sévérité | Nombre | Domaine principal |
|---|---|---|
| P0 — Critique (bloquant v0.2.0) | 3 | Sleep, Input, FD leak |
| P1 — Majeur (stabilité dégradée) | 5 | TSC, mremap, Ring, FD global, KPTI |
| P2 — Modéré (cohérence fonctionnelle) | 6 | IPC, Signal, Sched, AArch64… |
| P3 — Mineur (propreté / monitoring) | 4 | Stats, validate_fd, Calib… |

---

## P0 — CRITIQUE : Bloquants immédiats pour v0.2.0

---

### P0-01 — `sys_nanosleep` est une spin-boucle, pas un vrai blocage

**Fichier :** `kernel/src/syscall/table.rs` — `fn sys_nanosleep()`

**Description :**  
`sys_nanosleep` ne bloque jamais réellement le thread appelant. L'implémentation actuelle tourne en boucle `spin_loop()` avec un `cooperative_reschedule()` toutes les 1024 itérations jusqu'à ce que le deadline TSC soit atteint :

```rust
loop {
    if monotonic_ns() >= deadline { break; }
    if spins & 0x3ff == 0 {
        cooperative_reschedule();  // cède le CPU
    } else {
        core::hint::spin_loop();   // burn CPU
    }
}
```

**Conséquences :**
- Exosh et tous les serveurs qui appellent `nanosleep` consomment 100 % du CPU pendant la durée du sleep.
- Les C-states ACPI (`scheduler/energy/c_states.rs`) ne sont jamais atteints — le CPU ne dort jamais.
- Le thread `idle` ne s'exécute jamais quand un processus "dort".
- Un `top` affiché dans exosh montrera systématiquement des CPU à 100 % même à charge nulle.
- Incompatible avec tout programme userspace qui utilise `nanosleep` pour du rate-limiting ou du throttling (smoltcp, serveurs réseau…).

**Fix requis :**  
Câbler `nanosleep` sur une vraie `WaitQueue` + deadline timer via `scheduler/timer/`. Le TCB doit passer à `TaskState::Sleeping`, le deadline timer réveille via `WaitQueue::notify` à l'expiration. Le `cooperative_reschedule()` actuel n'est qu'un workaround de bring-up.

**Validation :** `exosh:/$ sleep 1` doit consommer ~0 % CPU pendant 1 seconde.

---

### P0-02 — Input clavier en polling actif (pas d'IRQ-driven wakeup)

**Fichiers :** `kernel/src/arch/x86_64/terminal.rs` — `fn poll_byte()` / `fn poll_byte_for_process()`  
**Lien :** `kernel/src/syscall/fs_bridge.rs` — `fn fs_read()` (fd == 0)

**Description :**  
Toute lecture sur `fd=0` (stdin) appelle `terminal::poll_byte_for_process()`, qui scrute activement les ports I/O `0x64` / `0x60` du contrôleur PS/2 dans une boucle de 64 itérations max (`KEYBOARD_DRAIN_LIMIT = 64`). Si aucune touche n'est pressée, la fonction retourne immédiatement `None`, et le syscall `sys_read` retourne `EWOULDBLOCK`. Exosh boucle alors en rappelant `sys_read(0)` sans délai — c'est un busy-poll à pleine vitesse.

```
exosh → sys_read(fd=0) → poll_byte() → inb(0x64/0x60) → None
     → retourne EWOULDBLOCK → exosh reboucle immédiatement
```

L'IRQ 1 (vecteur 33) est bien défini et routé via `dispatch_irq()`, mais **aucun handler ne transfert les scancodes PS/2 vers un buffer partagé** — le vecteur est enregistré dans la table IRQ mais aucun driver ne l'écoute en mode interrupt-driven pour alimenter `KEYBOARD`.

**Conséquences :**
- Le core CPU0 tourne à 100 % en permanence dès qu'exosh attend une frappe clavier.
- Problème P0-01 amplifié : même avec un vrai `nanosleep`, exosh reboucle dès le retour.
- La consommation énergétique rend le système inutilisable sur matériel réel.

**Fix requis :**  
1. Enregistrer un handler IRQ 1 qui alimente le buffer interne `KEYBOARD` à l'interruption.
2. Bloquer `sys_read(fd=0)` sur une `WaitQueue` liée au buffer clavier.
3. Le handler IRQ réveille la `WaitQueue` à chaque scancode reçu.

---

### P0-03 — Fuite de descripteurs OBJECT_TABLE lors de `execve`

**Fichier :** `kernel/src/process/lifecycle/exec.rs` — lignes 237-259

**Description :**  
Lors d'un `execve`, la séquence de fermeture des fds `O_CLOEXEC` retourne les handles sous forme d'un `Vec<u64>` (`closed_handles`), puis les abandonne silencieusement avec `drop(closed_handles)`. Le commentaire dans le code l'admet explicitement :

```rust
// NOTE: les handles sont simplement abandonnés ; fs/ les collectera via
// un mécanisme de GC de handles (hors scope de ce module).
drop(closed_handles);
```

Or ce mécanisme de GC n'existe pas. **`OBJECT_TABLE.close(fd)` n'est jamais appelé** pour ces handles lors d'un exec. Les entrées dans la table globale `OBJECT_TABLE` restent occupées indéfiniment. Avec `MAX_FDS = 65 532`, une succession d'exec suffit à saturer la table.

À noter : lors de l'exit (`process/lifecycle/exit.rs`), le chemin est correct — `close_all_noalloc()` + hook `vfs_close_all_pid(pid)` nettoient proprement. Le bug est **spécifique à exec + CLOEXEC**.

**Fix requis :**  
Dans `exec.rs`, après `close_on_exec()`, itérer sur les handles retournés et appeler explicitement `crate::fs::exofs::syscall::fs_bridge::fs_close(handle_as_fd, pid)` pour chaque handle.

---

## P1 — MAJEUR : Stabilité structurellement dégradée

---

### P1-01 — Chaîne de calibration TSC : HPET et PM Timer absents de l'exécution réelle

**Fichier :** `kernel/src/arch/x86_64/time/calibration/mod.rs` — `fn run_calibration_chain()`

**Description :**  
La documentation du module (lignes 14-18) décrit la chaîne de fallback ainsi :

```
1. HPET window 1ms × 10 samples    (rating 300)
2. PM Timer window 1ms × 10 samples(rating 200)
3. CPUID leaf 0x15                 (rating 150)
4. CPUID leaf 0x16                 (rating 100)
5. PIT one-shot 1ms × 10 samples   (rating  50)
6. Fallback 3 GHz                  (rating  10)
```

L'implémentation réelle de `run_calibration_chain()` est :

```
Tentative 1 → CPUID 0x15
Tentative 2 → CPUID 0x16
Tentative 3 → CPUID best-estimate
Tentative 4 → PIT driver
Fallback    → 3 GHz hardcodé
```

**HPET et PM Timer n'apparaissent nulle part** dans la chaîne exécutée. `CalibSource::Hpet` et `CalibSource::PmTimer` sont définis dans l'enum, apparaissent dans `is_real_measurement()` et `calibrated_with_hpet()`, mais ces fonctions retourneront **toujours `false`** car aucun chemin de code ne leur assigne jamais `LAST_SOURCE`. De même, `sources/hpet.rs` et `sources/pm_timer.rs` existent mais ne sont jamais appelés depuis la chaîne de calibration.

**Conséquence :** Sur QEMU TCG (où CPUID 0x15 ne retourne rien d'utile et où le PIT driver échoue), le système tombe systématiquement au fallback 3 GHz — confirmé par le log boot :  
`[CAL:PIT-DRV-FAIL][CAL:FB3G hz=3000000000][TIME-INIT hz=3000000000]`

Toute mesure temporelle (scheduler quanta, timeouts IPC, nanosleep) est affectée par l'imprécision si la fréquence réelle de la VM diffère de 3 GHz.

**Fix requis :**  
1. Insérer HPET et PM Timer entre PIT et fallback dans `run_calibration_chain()`, ou en tête si disponibles.  
2. Corriger la documentation pour refléter l'ordre réel, ou corriger l'implémentation pour correspondre à la doc.

---

### P1-02 — Double système de FD : namespace global non isolé par processus

**Fichiers :**  
- `kernel/src/fs/exofs/syscall/object_fd.rs` → `OBJECT_TABLE` (global, fd 4…65535)  
- `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs` → `FD_TABLE` (global, VFS_OPEN_MAX=1024)

**Description :**  
Il existe deux tables de descripteurs globales indépendantes :

- **`OBJECT_TABLE`** : table plate de 65 532 slots, partagée entre tous les processus. Les fds y sont alloués globalement (pas par PID). `OBJECT_TABLE.get(fd)` ne vérifie pas que le fd demandé appartient au processus appelant — la garde est uniquement que le slot existe.
- **`FD_TABLE` (posix_bridge)** : table de 1024 entrées globales max pour l'ensemble du système. `next_fd` est un compteur atomique monotoniquement croissant — les numéros fd ne sont **jamais réutilisés** après fermeture, seulement le slot physique dans le tableau. Après ~65K opens/closes cumulés sur le système, les numéros fd dépassent 65 535 et wrappent de manière incorrecte.

**Conséquences :**
- Un processus peut, s'il devine un fd numéro valide, accéder à un fichier ouvert par un autre processus (pas de vérification PID dans `OBJECT_TABLE.get()`).
- `VFS_OPEN_MAX = 1024` est la limite totale système, pas par processus. 13 serveurs + exosh = ~50-100 fds ouverts au boot. La marge est étroite pour des applications multi-fichiers.
- Le `next_fd` monotone de `FD_TABLE` peut saturer sur des systèmes à longue durée de vie.

**Fix requis :**  
- Ajouter une vérification `owner_pid` dans `OBJECT_TABLE.get()` (le champ `owner_uid` existe déjà).
- Ou remplacer `next_fd` monotone par un allocateur bitmap avec réutilisation.
- Augmenter `VFS_OPEN_MAX` ou le rendre configurable par processus.

---

### P1-03 — `sys_mremap` limité à 32 MB (IO_BUF_MAX) et copie via heap kernel

**Fichier :** `kernel/src/syscall/table.rs` — `fn sys_mremap()`

**Description :**  
`sys_mremap` alloue un nouveau mapping via `do_mmap`, copie les données de l'ancien mapping via un vecteur kernel (`zeroed_user_vec(copy_len)`), puis libère l'ancien. Si `copy_len > IO_BUF_MAX` (32 MB), la fonction retourne `E2BIG`.

Problèmes :
1. **Limite 32 MB** : les allocateurs système modernes (jemalloc, tcmalloc, musl) émettent régulièrement des `mremap` de plusieurs centaines de MB pour les arènes heap. Ces appels échoueront.
2. **Copie by-value via kernel heap** : la sémantique correcte de `mremap` est un remapping de page tables sans copie des données (réutilisation des frames physiques). L'implémentation actuelle copie physiquement les octets — coût O(n) en mémoire ET en temps au lieu de O(1).
3. **Double mapping transitoire** : pendant la copie, le contenu existe en double dans deux mappings physiques différents, doublant la pression mémoire.

**Fix requis :**  
Implémenter `mremap` par manipulation de page tables : déplacer les PTEs de l'ancien range vers le nouveau, ajuster les VMAs en conséquence, invalider les TLBs. Pas de copie de données.

---

### P1-04 — Terminologie "Ring 1" trompeuse : les serveurs tournent en Ring 3

**Fichiers :** GDT (`kernel/src/arch/x86_64/gdt.rs`), tous les `servers/*/src/main.rs`

**Description :**  
L'architecture et la documentation décrivent systématiquement les serveurs (ipc_router, vfs_server, memory_server…) comme des "serveurs Ring 1". Or le GDT définit **uniquement Ring 0 (kernel) et Ring 3 (user)** :

```
GDT_KERNEL_CS = 0x08  (Ring 0)
GDT_USER_CS32 = 0x18 | 3  (Ring 3)
GDT_USER_CS64 = 0x28 | 3  (Ring 3)
```

Il n'existe aucun segment Ring 1 ou Ring 2. Tous les serveurs (init_server, ipc_router, exo_shield, exosh…) s'exécutent au CPL 3 — le même niveau de privilège qu'un processus utilisateur lambda. Il n'y a aucune isolation hardware supplémentaire entre un serveur "Ring 1" et une application quelconque.

L'isolation réelle repose exclusivement sur le système de capabilities (capabilities tokens + security gate) et sur la séparation des espaces d'adressage — pas sur les anneaux CPU.

**Conséquence :** Toute documentation, présentation, ou argument de sécurité reposant sur "Ring 1 = isolation hardware" est incorrect et peut induire en erreur les reviewers de sécurité ou les auditeurs externes (NLnet, STF…).

**Fix requis :**  
- Renommer partout "Ring 1 servers" → "Privilege-Separated Servers (Ring 3 + capability-isolated)".  
- Ou implémenter une vraie séparation Ring 1 (segment Ring 1, change de CPL au syscall vers les serveurs). Complexité élevée.

---

### P1-05 — KPTI : switch CR3 user incomplet dans les stubs syscall/exception

**Fichiers :**  
- `kernel/src/arch/x86_64/spectre/kpti.rs` — `kpti_switch_to_user()` / `kpti_switch_to_kernel()`  
- `kernel/src/arch/x86_64/syscall.rs`  
- `kernel/src/scheduler/core/switch.rs`

**Description :**  
Le context switch (`switch.rs`) passe `next.cr3_phys` (CR3 kernel) à l'ASM `context_switch_asm`. Le CR3 user shadow est stocké dans le slot per-CPU KPTI via `set_current_cr3(next.cr3_phys, user_cr3)`, mais `kpti_switch_to_user()` et `kpti_switch_to_kernel()` ne sont **jamais appelés** depuis les stubs de syscall entry/exit :

```bash
$ grep -rn "kpti_switch" kernel/src/ | grep -v "kpti.rs\|mod.rs"
# → aucun résultat
```

Les fonctions `switch_to_user(cpu_id)` et `switch_to_kernel(cpu_id)` existent dans `kpti_split.rs` mais ne sont pas non plus câblées aux stubs ASM. Le commentaire dans `syscall.rs` indique "Le switch CR3 se fait dans switch_asm.s", mais `switch_asm.s` reçoit `new_cr3` = CR3 kernel, jamais le CR3 user shadow.

**Conséquence :** Si KPTI est activé (via `apply_mitigations_bsp()`), le CR3 ne switche pas vers la vue user restreinte lors du SYSRET / IRETQ vers Ring 3. La protection Meltdown est ineffective — le kernel est entièrement mappé dans l'espace adressable Ring 3.

**Fix requis :**  
Dans les stubs ASM de syscall entry/exit et dans les stubs d'exception handler (`define_exception_handler_*`), ajouter les `mov cr3, <user_cr3>` / `mov cr3, <kernel_cr3>` appropriés conditionnels à `KPTI_ENABLED`, en utilisant les valeurs per-CPU stockées.

---

## P2 — MODÉRÉ : Cohérence fonctionnelle à corriger pour v0.2.0

---

### P2-01 — `input_server` → `tty_server` : le forwarding des bytes clavier n'est pas câblé

**Fichiers :** `servers/input_server/src/main.rs`, `servers/tty_server/src/main.rs`

**Description :**  
`input_server` accumule des `InputEventWire` et répond aux messages `INPUT_MSG_POLL`. `tty_server` accepte des `TTY_MSG_INPUT_BYTE` pour alimenter la `LineDiscipline`. Mais **aucun code** ne transmet les événements de `input_server` vers `tty_server` : ni un thread de forwarding, ni un IPC envoyé par le kernel lors d'une IRQ clavier.

Le chemin actuel qui fonctionne en boot est : kernel IRQ 1 → `terminal::poll_byte()` → `fs_read(fd=0)` — en bypass complet des deux serveurs. Les serveurs `input_server` et `tty_server` démarrent, répondent aux heartbeats, mais ne reçoivent et ne transmettent aucune donnée clavier réelle.

**Conséquence :** Toute la pile `input_server → tty_server → exosh` est une coquille vide. Pour Wayland (v0.3.0), cette pile sera nécessaire.

**Fix requis :**  
Câbler le handler IRQ 1 kernel pour envoyer les bytes PS/2 décodés à `input_server` via IPC. `input_server` forward vers `tty_server` via `TTY_MSG_INPUT_BYTE`. `tty_server` bloque `sys_read(fd=0)` sur une wait queue réveillée par la LineDiscipline.

---

### P2-02 — `SIGPIPE` non émis sur écriture dans un tube brisé

**Fichier :** `kernel/src/syscall/fs_bridge.rs` — section pipe write (pseudo-blobs `PSEUDO_PIPE_TAG`)

**Description :**  
Lors d'une écriture dans un pipe pseudo-blob dont le lecteur est mort, `fs_write()` retourne `FsBridgeError::BadFd` ou équivalent — converti en `EPIPE` au niveau syscall. Mais **`SIGPIPE` n'est jamais envoyé au processus écrivant**.

La sémantique POSIX exige que si `SA_IGN` n'est pas positionné pour `SIGPIPE`, le processus reçoive le signal **avant** que l'errno `EPIPE` soit retourné. Les programmes qui comptent sur `SIGPIPE` pour terminer un pipeline shell (`cmd1 | cmd2` où `cmd2` se termine avant `cmd1`) ne fonctionneront pas correctement.

**Fix requis :**  
Dans `fs_write()`, quand le pipe est détecté brisé, appeler `send_signal_to_pid(writer_pid, SIGPIPE)` avant de retourner `EPIPE`.

---

### P2-03 — AArch64 : porte placeholder dans un codebase présenté comme multi-arch

**Fichier :** `kernel/src/arch/aarch64/mod.rs`

**Description :**  
Le module `arch/aarch64` est explicitement un placeholder (`"Placeholder — l'implémentation complète sera réalisée lors du portage AArch64"`). Il fournit uniquement `read_tsc()` (CNTVCT_EL0), `halt_cpu()`, `irq_disable()`, `irq_enable()`. Tout le reste — IDT/GIC, GDT/EL0→EL1 switch, APIC/GIC, memory map, boot — est absent.

Le `build.rs` et `Cargo.toml` ne bloquent pas la compilation pour cible `aarch64-unknown-none`. Un `cargo build --target aarch64-unknown-none` compilera un binaire non-fonctionnel sans avertissement explicite.

**Conséquence :** Le pitch (PITCH_ONE_PAGER.md) et la documentation présentent ExoOS comme une architecture x86_64. La présence du module aarch64 peut créer une fausse impression de support multi-architecture pour les évaluateurs.

**Fix requis :**  
Soit supprimer le module aarch64 jusqu'au vrai portage, soit ajouter une garde de compilation explicite (`#[cfg(target_arch = "x86_64")]` sur tous les `mod arch`) et un `compile_error!` clair si on tente de builder pour aarch64.

---

### P2-04 — `memory_server` : validation des handles de région insuffisante

**Fichier :** `servers/memory_server/src/mmap_service.rs` — `fn handle_free()`, `fn handle_protect()`

**Description :**  
`handle_free()` cherche la région par `handle` numérique et vérifie `owner_pid`. Mais le handle est un `u64` incrémenté séquentiellement (`next_handle`). Un processus malveillant peut deviner les handles d'autres processus par force brute (plage de 1 à quelques milliers sur un système courant).

De plus, `handle_protect()` appelle `SYS_MPROTECT` sur `region.base_addr` — l'adresse virtuelle dans l'espace du memory_server lui-même (Ring 3), pas dans l'espace du processus demandeur. Un `MEMORY_MSG_PROTECT` envoyé par le processus A modifie les permissions dans l'espace du memory_server, pas dans l'espace d'A.

**Fix requis :**  
- Utiliser des handles opaques (hash/token non prévisibles) au lieu de compteurs séquentiels.
- `handle_protect` doit cibler l'espace d'adressage du processus demandeur, pas celui de memory_server.

---

### P2-05 — Pas de load balancing entre CPUs pour les nouveaux processus

**Fichier :** `kernel/src/process/lifecycle/create.rs` — ligne 390

**Description :**  
`create_process()` enfile toujours le nouveau processus sur `run_queue(CpuId(0))`. Tous les 13 serveurs au boot + exosh sont enfilés sur CPU 0. Sur un système SMP (QEMU -smp 2+), tous les threads applicatifs démarrent et s'exécutent sur un seul cœur.

`fork()` améliore légèrement la situation en utilisant `ctx.target_cpu = tcb.current_cpu()` (le CPU du parent), mais ne fait aucun load balancing.

Le module de load balancing (`scheduler/core/runqueue.rs` — `cfs_dequeue_for_migration`) existe mais n'est pas appelé automatiquement lors de la création de processus.

**Fix requis :**  
Dans `create_process()`, choisir le CPU cible en minimisant `run_queue(cpu).len()` parmi tous les CPUs actifs, au lieu de toujours cibler CPU 0.

---

### P2-06 — `close_on_exec` : fuite des fds VFS (FD_TABLE) sans appel à `vfs_close_all_pid`

**Fichier :** `kernel/src/process/lifecycle/exec.rs`

**Description :**  
Lors d'un exec, `close_on_exec()` retire les fds `O_CLOEXEC` de la table PCB. Mais les entrées correspondantes dans `FD_TABLE` (posix_bridge, la table VFS globale) ne sont pas fermées. La fermeture VFS correcte passe par `FD_TABLE.close_fd(fd_number)`. Or ni `fs_bridge::fs_close()` ni `FD_TABLE.close_fd()` ne sont appelés sur ces handles.

Contrairement à l'exit (qui appelle `vfs_close_all_pid(pid)`), exec n'a aucun équivalent partiel pour les fds CLOEXEC dans la couche VFS.

**Fix requis :**  
Dans le chemin exec, après `close_on_exec()`, appeler `fs_bridge::fs_close(fd, pid)` pour chaque fd numérique retiré (récupérable via la liste des handles + recherche dans FD_TABLE par pid).

---

## P3 — MINEUR : Propreté et monitoring

---

### P3-01 — `calibrated_with_hpet()` retourne toujours `false`

**Fichier :** `kernel/src/arch/x86_64/time/calibration/mod.rs` — `fn calibrated_with_hpet()`

**Description :**  
`calibrated_with_hpet()` vérifie `LAST_SOURCE == CalibSource::Hpet`. Comme `run_calibration_chain()` n'assigne jamais `CalibSource::Hpet` à `LAST_SOURCE` (HPET absent de la chaîne — voir P1-01), cette fonction est de facto une constante `false`. Tout code conditionnel sur `calibrated_with_hpet()` (drift correction, monitoring) est inopérant.

**Fix :** Corriger conjointement avec P1-01, ou documenter explicitement que HPET n'est pas supporté.

---

### P3-02 — `CALIB_SEQ` indistinguable entre calibration initiale et re-calibration

**Fichier :** `kernel/src/arch/x86_64/time/calibration/mod.rs`

**Description :**  
`calibrate_tsc()` et `recalibrate_tsc()` incrémentent tous deux `CALIB_SEQ`. Le monitoring post-boot (`calibration_stats()`) ne peut pas distinguer "séquence 3 = 3ème appel initial" de "séquence 3 = 1 initial + 2 re-calibrations". `RECALIB_COUNT` existe séparément mais n'est pas exposé dans `calibration_stats()`.

**Fix :** Exposer `RECALIB_COUNT` dans `calibration_stats()` ou séparer les séquences.

---

### P3-03 — `validate_fd` ne consulte pas la table PCB du processus courant

**Fichier :** `kernel/src/syscall/validation.rs` — `fn validate_fd()`

**Description :**  
```rust
pub fn validate_fd(raw: u64) -> Result<i32, SyscallError> {
    if raw > 65535 { return Err(SyscallError::Invalid); }
    Ok(raw as i32)
}
```

`validate_fd` valide uniquement la plage numérique (0…65535) sans vérifier que le fd existe dans la table du processus courant (`pcb.files.lock().get(fd)`). La vérification réelle est déléguée à `OBJECT_TABLE.get(fd)` ou `FD_TABLE.get_fd(fd)` en aval. Un fd numériquement valide mais non ouvert par le processus courant passe `validate_fd` sans erreur.

Ce n'est pas un bug exploitable immédiatement (les tables globales filtrent l'accès), mais c'est une défense en profondeur manquante qui devrait être corrigée avant un usage multi-process sérieux.

**Fix :** Ajouter une vérification `pcb.files.lock().descriptors[fd_num].is_some()` dans `validate_fd`.

---

### P3-04 — ExoFS GC kthread : priorité IDLE sur CPU 0 uniquement

**Fichier :** `kernel/src/fs/exofs/mod.rs` — `exofs_gc_kthread`

**Description :**  
Le kthread GC ExoFS est créé avec `priority: Priority::IDLE` et `target_cpu: 0`. En conséquence :
- Le GC ne s'exécute que quand CPU 0 est complètement idle — ce qui n'arrive jamais si exosh tourne en busy-poll (P0-02).
- Sur SMP, le GC ne migre jamais sur un autre CPU même si CPU 0 est saturé.
- Les blobs orphelins et les epochs expirées s'accumulent sans être collectés, dégradant progressivement les performances ExoFS.

**Fix :** Passer le GC en `priority: Priority::LOW` (pas IDLE) et activer la migration inter-CPU, ou le déclencher sur deadline (timer périodique) plutôt que sur préemption idle.

---

## Synthèse pour v0.2.0

Le kernel ExoOS v0.1.0 est **fonctionnel en régime de démonstration QEMU** (boot, exosh, commandes de base). Pour atteindre une stabilisation complète v0.2.0, le plan de correction prioritaire est :

### Phase A — Corrections bloquantes (avant tout le reste)
| ID | Action | Effort estimé |
|---|---|---|
| P0-01 | Câbler nanosleep sur WaitQueue + hrtimer | ~2j |
| P0-02 | IRQ-driven keyboard + wakeup sys_read(0) | ~1.5j |
| P0-03 | Fermer OBJECT_TABLE sur exec CLOEXEC | ~0.5j |

### Phase B — Stabilité structurelle
| ID | Action | Effort estimé |
|---|---|---|
| P1-01 | Réintégrer HPET/PMT dans run_calibration_chain | ~1j |
| P1-02 | Vérification owner_pid dans OBJECT_TABLE.get() | ~0.5j |
| P1-03 | mremap par page-table manipulation (sans copie) | ~2j |
| P1-05 | KPTI : câbler switch CR3 user dans stubs ASM | ~1j |

### Phase C — Cohérence (peut aller en parallèle)
| ID | Action | Effort estimé |
|---|---|---|
| P1-04 | Renommer "Ring 1" → "Privilege-Separated" (doc) | ~2h |
| P2-01 | Câbler IRQ1 → input_server → tty_server | ~2j |
| P2-02 | Émettre SIGPIPE sur pipe brisé | ~0.5j |
| P2-03 | Garde de compilation pour aarch64 | ~0.5j |
| P2-05 | Load balancing création processus | ~1j |
| P2-06 | Fermer FD_TABLE sur exec CLOEXEC | ~0.5j |

### Phase D — Propreté (post-v0.2.0 si besoin)
P3-01, P3-02, P3-03, P3-04 — tous < 1 jour chacun.

---

## Annexe — Fichiers audités

| Module | Fichier principal |
|---|---|
| Calibration TSC | `kernel/src/arch/x86_64/time/calibration/mod.rs` |
| Fault handler | `kernel/src/arch/x86_64/exceptions.rs` |
| Memory interface | `kernel/src/arch/x86_64/memory_iface.rs` |
| Context switch | `kernel/src/scheduler/core/switch.rs` |
| Fork | `kernel/src/process/lifecycle/fork.rs` |
| Exec | `kernel/src/process/lifecycle/exec.rs` |
| Exit | `kernel/src/process/lifecycle/exit.rs` |
| Syscall table | `kernel/src/syscall/table.rs` |
| FS bridge | `kernel/src/syscall/fs_bridge.rs` |
| OBJECT_TABLE | `kernel/src/fs/exofs/syscall/object_fd.rs` |
| VFS FD_TABLE | `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs` |
| Terminal/PS2 | `kernel/src/arch/x86_64/terminal.rs` |
| KPTI | `kernel/src/arch/x86_64/spectre/kpti.rs` |
| IRQ routing | `kernel/src/arch/x86_64/irq/routing.rs` |
| GDT | `kernel/src/arch/x86_64/gdt.rs` |
| AArch64 | `kernel/src/arch/aarch64/mod.rs` |
| Servers | `servers/*/src/main.rs` (×11) |
| CoW fault | `kernel/src/memory/virtual/fault/cow.rs` |
| fork_impl | `kernel/src/memory/virtual/address_space/fork_impl.rs` |

---

*`claude-gamma` — Audit ExoOS v0.2.0 — 2026-05-14*  
*Aucune correction automatique appliquée. Ce document est un rapport d'audit pur.*
