# ExoOS — Audit des Incohérences Kernel pour la Stabilisation v0.2.0

**Rédigé par** : Claude Delta  
**Date** : 2026-05-14  
**Base** : ExoOS v0.1.0 "Elder and Bobby" — kernel.zip (snapshot 2026-05-13)  
**Objectif** : identifier toutes les incohérences bloquantes, majeures et mineures avant la release de stabilisation v0.2.0, préalable au portage Wayland et à l'installation visuelle.

---

## Sommaire des gravités

| Gravité | Nombre | Impact |
|---------|--------|--------|
| **P0 — Bloquant** | 4 | Correctif obligatoire avant v0.2.0 |
| **P1 — Majeur** | 6 | À résoudre dans le cycle v0.2.0 |
| **P2 — Mineur** | 6 | À documenter / planifier |

---

## P0 — Incohérences Bloquantes

### P0-1 · Contradiction dans l'ordre de verrouillage des locks

**Fichiers concernés** : `kernel/src/lib.rs:20`, `kernel/src/syscall/mod.rs:40`, `kernel/src/ipc/mod.rs` (commentaire canonique)

**Constat** :

`lib.rs` (règle architecturale de référence) déclare :
```
Lock ordering : Memory → Scheduler → Security → IPC → FS
```

`syscall/mod.rs` (règle regle_bonus reprise dans la doc du module) déclare :
```
ordonnancement des verrous IPC < Sched < Mem < FS
```

La notation `IPC < Sched < Mem` signifie que IPC est acquis **en premier**, avant Scheduler et Memory. C'est l'**inverse exact** de l'ordre canonique. La source de référence est `ipc/mod.rs` qui confirme le bon ordre :
```
Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5)
```

**Risque** : Tout développeur lisant `syscall/mod.rs` comme source de vérité respectera un ordre inversé, ouvrant une fenêtre de deadlock entre les layers IPC et mémoire sur chemin SMP.

**Correction** : Uniformiser le libellé dans `syscall/mod.rs` pour lire :
```
ordonnancement des verrous : Memory < Sched < Security < IPC < FS
(= Memory acquis en premier, FS acquis en dernier — ordre canonique ExoOS)
```

---

### P0-2 · `SYSCALL_TABLE_SIZE` : documentation et valeur incohérentes

**Fichier** : `kernel/src/syscall/numbers.rs:8–31`

**Constat** :

Le bloc de documentation en tête du fichier déclare :
```
- [400..511] : réservés pour usage futur
- 512        : SYSCALL_TABLE_SIZE (taille totale de la table)
```

La constante réelle est :
```rust
pub const SYSCALL_TABLE_SIZE: usize = 547;
// "547 = couvre POSIX (0–499) + ExoFS (500–520) + GI-03 drivers (530–546)."
```

Conséquences :
- La plage `[400..511]` annoncée comme réservée est en réalité peuplée : `SYS_EXOFS_RELATION_CREATE = 512`, et des syscalls ExoFS occupent 500–520 + drivers GI-03 à 530–546.
- La borne `512` donnée comme taille de table est fausse de 35 entrées.
- Tout outil externe générant du code depuis cette documentation (libc stubs, ABI contract tests) produira une table trop petite, laissant les handlers ExoFS/GI-03 hors borne avec retour `ENOSYS` silencieux.

**Correction** : Mettre à jour le bloc de documentation pour refléter :
```
- [300..399] : extensions Exo-OS natifs (IPC, capabilities, sécurité)
- [400..499] : réservés
- [500..520] : ExoFS syscalls natifs
- [521..529] : réservés
- [530..546] : GI-03 drivers syscalls
- 547        : SYSCALL_TABLE_SIZE
```

---

### P0-3 · `cmd_top` / `ps` : sondage PID par `kill(0)` et table de noms codée en dur

**Fichiers** : `servers/exosh/src/main.rs:719–750`, `kernel/src/syscall/numbers.rs` (absence de `SYS_EXO_PROCESS_LIST`)

**Constat** :

`cmd_top()` implémente la liste des processus en sondant les PIDs 1 à 64 via `kill(pid, 0)` et en affichant des noms depuis une table statique `known_process_name()` :

```rust
fn known_process_name(pid: u32, self_pid: u32) -> &'static [u8] {
    match pid {
        1 => b"init_server",
        2 => b"ipc_router",
        // ...
        12 => b"exo_shield",   // ← potentiellement inversé (voir P1-3)
        13 => b"exosh",
        _ if pid == self_pid => b"exosh",
        _ => b"user_process",
    }
}
```

Problèmes :
1. Aucun syscall `SYS_EXO_PROCESS_LIST` n'existe dans `numbers.rs`. Le kernel n'expose pas de primitive pour lister les processus vivants.
2. La table de noms sera fausse dès qu'un service crashe et est relancé sous un nouveau PID.
3. Tout processus utilisateur hors PID 1–13 s'affiche comme `user_process` sans nom réel.
4. Le sondage via `kill(0)` sur 64 PIDs génère 64 syscalls pour chaque invocation de `top`.

La note de clôture v0.1.0 confirme : *"top utilise encore une table PID/nom connue côté shell ; la prochaine étape est un vrai syscall de liste de processus."*

**Correction v0.2.0** :
- Ajouter `SYS_EXO_PROCESS_LIST` dans `numbers.rs` (slot à réserver en zone 300–399).
- Implémenter le handler dans `kernel/src/process/core/registry.rs` via `for_each()` existant.
- Modifier `cmd_top()` pour appeler ce syscall et afficher les noms réels depuis le PCB.

---

### P0-4 · `PhoenixState` kernel découplé du `PhoenixPhase` du network server

**Fichiers** : `kernel/src/exophoenix/mod.rs:20–32`, `servers/network_server/src/isolation.rs:6–12`

**Constat** :

Le kernel expose un enum `PhoenixState` à 9 états avec un global atomique `PHOENIX_STATE` :
```rust
pub enum PhoenixState {
    BootStage0 = 0, Normal = 1, Threat = 2,
    IsolationSoft = 3, IsolationHard = 4, Certif = 5,
    Restore = 6, Degraded = 7, Emergency = 8,
}
pub static PHOENIX_STATE: AtomicU8 = AtomicU8::new(PhoenixState::BootStage0 as u8);
```

Le `network_server` définit son propre enum local, **sans aucun lien** avec le global kernel :
```rust
pub enum PhoenixPhase {
    Normal,
    Draining,
    Serialized,
}
```

L'audit EXONET V4 documente les transitions `Normal → Draining → Serialized → Normal` mais elles ne se reflètent pas dans `PHOENIX_STATE`. Le kernel ne sait pas que le network server est en train de drainer ou est sérialisé. Aucun IPC ni MSR ne synchronise ces états.

**Risque** : Si le kernel déclenche un `IsolationHard` ou `Emergency` pendant que le network server est en phase `Draining`, les deux machines d'état partent dans des directions incompatibles, pouvant laisser des buffers orphelins ou déclencher un double-drain.

**Correction** :
- Soit enrichir `PhoenixState` des états `Draining` et `Serialized`.
- Soit faire envoyer au network server un message IPC vers le kernel pour mettre à jour `PHOENIX_STATE` lors de ses transitions.
- Documenter explicitement que `PhoenixPhase` est un état local interne au réseau, intentionnellement découplé, avec justification.

---

## P1 — Incohérences Majeures

### P1-1 · Le crate `loader` est un stub non-fonctionnel

**Fichiers** : `loader/src/main.rs`, `loader/src/dynamic_linker/`

**Constat** :

L'entry point `_start` du loader est une boucle vide :
```rust
pub extern "C" fn _start() -> ! {
    loop { core::hint::spin_loop(); }
}
```

Le module `dynamic_linker/` est composé de squelettes de types sans logique :
- `library.rs` : struct `LibraryRef` vide
- `resolver.rs` : enum `ResolveError { NotFound, Ambiguous }` seulement
- `search_path.rs` : constante `DEFAULT_LIBRARY_PATHS` seulement
- `symbol_table.rs` : struct `SymbolRef` vide
- `version.rs` : constante `LOADER_ABI_VERSION = 1`

Le kernel charge les ELF via `kernel/src/fs/elf_loader_impl.rs` directement ; le crate `loader/` n'est jamais invoqué au boot. Le linkage dynamique (`.so`, `PT_INTERP`, relocations `R_X86_64_*`) n'existe pas dans ExoOS à ce stade.

**Impact** : Le `loader/` crée une fausse impression de support dynamique. Toute tentative d'exécuter un binaire lié dynamiquement résultera en spin infini silencieux.

**Correction** : Soit supprimer le crate `loader/` et documenter l'absence de dynamic linking en v0.2.0, soit le marquer `#[cfg(feature = "dynamic_linking")]` désactivé par défaut avec une note claire dans le README.

---

### P1-2 · Deux implémentations `exosh` sans gate de build

**Fichiers** : `servers/exosh/src/main.rs` (74 KB, no_std, opérationnel), `userspace/apps/exosh/src/main.rs` (std, hôte uniquement)

**Constat** :

`userspace/apps/exosh/src/main.rs` utilise `std::process::Command`, `std::env`, `std::io` — aucune de ces primitives n'existe dans ExoOS bare-metal. Ce binaire est un prototype hôte Linux. Il ne peut pas être compilé pour `x86_64-unknown-none`.

`servers/exosh/` est le vrai shell embarqué, la seule implémentation opérationnelle. Il n'y a aucun commentaire, feature flag, ou documentation indiquant la distinction.

**Impact** : Un contributeur cherchant à modifier le shell pourrait modifier `userspace/apps/exosh/` sans effet sur le système réel. Confusion garantie lors du portage Wayland où un vrai `userspace/` sera nécessaire.

**Correction** :
- Renommer `userspace/apps/exosh/` en `host_tools/exosh_prototype/` ou le déplacer hors du workspace `userspace/Cargo.toml`.
- Ajouter un commentaire explicite dans le `README.md` racine : *"Le shell embarqué est `servers/exosh/`. `userspace/apps/exosh/` est un prototype hôte non embarqué."*

---

### P1-3 · Bug « top PID name swap » : exosh/exo_shield inversés

**Fichiers** : `servers/init_server/src/main.rs:91–103`, `servers/exosh/src/main.rs:2088–2106`, `servers/init_server/src/service_table.rs` (`DEPS_EXOSH`)

**Constat** :

La table `SERVICES` dans `init_server/src/main.rs` déclare les services dans l'ordre :
```
index 10 → exosh        (PID 12 présumé)
index 11 → exo_shield   (PID 13 présumé)
```

Mais `DEPS_EXOSH = ["tty_server", "vfs_server", "exo_shield"]` : **exosh dépend de exo_shield**. Le système de dépendances force `exo_shield` à démarrer et devenir ready *avant* `exosh`. Donc le PID effectif est :
```
exo_shield → PID 12 (démarre en premier)
exosh      → PID 13 (démarre après)
```

Or `known_process_name()` dans exosh mappe :
```
12 → b"exo_shield"   ← cohérent avec la réalité si l'ordre de deps est respecté
13 → b"exosh"
```

La table semble correcte du point de vue des dépendances, **mais** si init_server essaie de démarrer les services dans l'ordre `SERVICES` sans attendre les deps (race condition ou timeout), exosh peut démarrer avant exo_shield et obtenir PID 12. Le bug est documenté dans `PLAN_POSTSHELL_FIXES.md` comme FIX-3 ("top PID name swap") et observé en production QEMU.

**Correction** :
- Réorganiser l'array `SERVICES` pour refléter l'ordre de démarrage réel (exo_shield avant exosh, en accord avec les dépendances).
- À terme, remplacer la table statique par le syscall `SYS_EXO_PROCESS_LIST` (P0-3).

---

### P1-4 · TSC : fallback `FB3G` observable en production

**Fichiers** : `kernel/src/arch/x86_64/time/calibration/mod.rs:252–350`, `docs/special/1/PLAN_POSTSHELL_FIXES.md` (FIX-2)

**Constat** :

La chaîne de calibration TSC est dans l'ordre :
1. CPUID leaf 0x15 (ratio TSC/cristal)
2. CPUID leaf 0x16 (fréquence base)
3. CPUID best estimate
4. PIT one-shot
5. Fallback 3 GHz codé en dur

Le log de v0.1.0 montre `[CAL:PIT-DRV hz=2614777097]` — le PIT est atteint (tentative 4), ce qui signifie que CPUID 0x15 et 0x16 ont échoué ou retourné 0 sur la configuration QEMU TCG utilisée. Des runs antérieurs affichent `[CAL:FB3G]` (fallback 5, 3 GHz nominal).

Avec `FB3G`, si la fréquence réelle de l'hôte est 3.2 GHz, `ktime_get_ns()` dérive de ~6%, impactant :
- Les quanta scheduler (sous-estimation des timeslices)
- Les timeouts IPC et watchdog
- Tous les benchmarks `time`, `dd`
- La précision du `SYS_CLOCK_GETTIME`

**Correction** :
- Vérifier pourquoi CPUID 0x15 échoue sur QEMU TCG (QEMU expose `TSC_DEADLINE` mais pas toujours la leaf 0x15 selon la version).
- Ajouter en fallback entre CPUID et PIT : lecture de `QEMU_TSC_KHZ` via `fw_cfg` si disponible.
- La note du plan (FIX-2) suggère de remonter CPUID 0x15 — vérifier qu'il est bien en première position (il l'est déjà dans le code) et diagnostiquer pourquoi il ne réussit pas.

---

### P1-5 · Binaires embarqués non strippés : ~37 MB de payload

**Fichiers** : `Makefile` (non inclus dans le zip), `docs/special/1/PLAN_POSTSHELL_FIXES.md` (FIX-1)

**Constat** :

Le plan post-shell documente les tailles actuelles :

| Binaire | Taille |
|---------|--------|
| exo-crypto-server | 6.3 MB |
| exo-shield | 3.5 MB |
| exosh | 2.6 MB |
| 10 autres binaires | ~2.5–2.9 MB chacun |
| **Total payload** | **~37 MB** |

Ces binaires embarquent leurs symboles debug. Conséquences :
- Chargement ELF au boot : ~3–5 s estimés
- Seulement ~475 MB libres dans l'image ExoFS sur 512 MB
- L'`fsck` ExoFS est proportionnel aux blocs alloués

La commande `strip` sur les binaires Rust ramènerait chaque binaire à 200–500 KB, soit **~5–6 MB total**.

**Correction** : Ajouter une passe `strip` dans le pipeline de build Makefile après `cargo build --release` et avant injection dans l'image ExoFS. FIX-1 est décrit mais non encore appliqué.

---

### P1-6 · Couverture de tests ExoFS non prouvée

**Fichier** : `kernel/src/fs/exofs/tests/TESTS_STATUS_REPORT.md`

**Constat** :

Le `TESTS_STATUS_REPORT.md` embarqué dans le repository déclare explicitement :

> **État réel de validation par tests observable ici : 0% prouvé.**

Ce rapport indique qu'au moment où il a été généré, aucun fichier de test n'était accessible, et donc :
- Les tiers d'intégration `tier_2` à `tier_6` ne sont pas vérifiés comme testant un vrai backend de stockage vs des mocks RAM.
- `tier_6_virtio_vfs.rs` (VFS réel + VirtIO) peut être un test d'interface seulement.
- Aucune assertion de persistance après remount ou reboot simulé n'est prouvée.

**Risque v0.2.0** : La stabilisation du kernel repose sur ExoFS comme filesystem de boot. Si les tests d'intégration ExoFS sont des mocks, des régressions de persistance resteraient invisibles pendant les cycles de développement.

**Correction** :
- Exécuter `cargo test` pour les crates ExoFS avec un vrai virtio_blk simulé (ou via `qemu-system` + `--kernel` en mode test).
- Documenter dans `TESTS_STATUS_REPORT.md` quels tiers utilisent des mocks vs un vrai block device.
- Bloquer les merges v0.2.0 sur réussite de `tier_4_pipeline` et `tier_6_virtio_vfs` en CI.

---

## P2 — Incohérences Mineures

### P2-1 · Architecture aarch64 déclarée comme placeholder explicite

**Fichier** : `kernel/src/arch/aarch64/mod.rs:8`

> *"Placeholder — l'implémentation complète sera réalisée lors du portage AArch64."*

La cible aarch64 compile (primitives minimales : `read_tsc`, `halt_cpu`, `irq_disable`) mais aucun GDT/IDT équivalent (VBAR, EL1), aucun APIC ni GIC, pas de SMP, pas de syscall AArch64. Le kernel ne peut pas booter sur AArch64.

**Action** : Retirer aarch64 de la liste des architectures supportées dans tous les documents officiels jusqu'au portage effectif. Ajouter `#[cfg(not(target_arch = "aarch64"))]` sur les modules critiques pour éviter des compilations trompeuses.

---

### P2-2 · `#![allow(static_mut_refs)]` global dans `lib.rs`

**Fichier** : `kernel/src/lib.rs:28`

```rust
#![allow(static_mut_refs)]
```

Ce lint supprime les avertissements Rust sur les références mutables vers des statics globaux — une source majeure de UB en code concurrent. Son application globale masque des accès potentiellement non synchronisés dans l'ensemble du crate.

**Action** : Identifier les sites qui déclenchent ce lint (`grep -n "mut.*static\|&mut.*STATIC"` dans le codebase) et les corriger un par un avec `AtomicPtr`, `UnsafeCell` documenté avec `// SAFETY:`, ou des abstractions `spin::Mutex`. Supprimer le `allow` global une fois tous les sites corrigés.

---

### P2-3 · `virtio_drivers` server : boucle IPC heartbeat-only

**Fichier** : `servers/virtio_drivers/src/main.rs`

Le serveur `virtio_drivers` ne traite que `VIRTIO_MSG_HEARTBEAT` et `VIRTIO_MSG_STATUS`. Il ne gère aucun protocole VirtIO réel (virtqueue ring, descripteurs DMA, interruptions). Le vrai protocole virtio_blk est directement dans `drivers/storage/virtio_blk/src/` côté kernel, sans passer par ce serveur.

**Risque** : La présence d'un serveur `virtio_drivers` vide crée une confusion architecturale. Si un futur développeur essaie de passer du I/O block device via ce serveur, il obtiendra un timeout silencieux.

**Action** : Soit supprimer `virtio_drivers` comme serveur Ring1 distinct et documenter que virtio_blk est géré directement dans le kernel, soit implémenter le protocole VQ complet dans ce serveur.

---

### P2-4 · Crates drivers entièrement vides

**Répertoires concernés** : `drivers/storage/ahci/src/`, `drivers/storage/nvme/src/`, `drivers/network/e1000/src/`, `drivers/display/virtio_gpu/src/`, `drivers/clock/src/`

Tous ces fichiers ont une taille de 0 octet. Ils sont membres Cargo du workspace mais ne contiennent aucun code. Leur présence crée des membres de workspace compilés pour rien et des Cargo.lock pollués.

**Action** : Déplacer les crates vides dans `drivers/future/` avec un `Cargo.toml` marqué `publish = false` et une note `# Not yet implemented`. Retirer du workspace principal pour alléger le build.

---

### P2-5 · `userspace/apps/coreutils/` redondant avec les built-ins exosh

**Fichier** : `userspace/apps/coreutils/src/bin/` (cat, echo, ls, mkdir, rm, rmdir, touch)

Ces binaires existent dans le crate coreutils mais aucun payload boot ne les embarque dans l'image ExoFS. Toutes ces commandes sont implémentées comme built-ins dans `servers/exosh/src/main.rs`. Deux implémentations de `ls`, `cat`, `echo` etc. coexistent sans synchronisation de comportement.

**Action** : Décider d'une stratégie : soit les built-ins exosh deviennent des appels vers les binaires coreutils (nécessite `fork/exec` complet), soit les coreutils sont supprimés comme code mort et le shell reste monolithique pour v0.2.0.

---

### P2-6 · `COW_TABLE_SIZE = 65536` : saturation silencieuse

**Fichier** : `kernel/src/memory/cow/tracker.rs:18`

```rust
pub const COW_TABLE_SIZE: usize = 65536;
```

La table de hash CoW a une capacité fixe de 65 536 entrées. En cas de saturation (après de nombreux `fork()`s imbriqués ou mmap shared intensif), les nouvelles allocations CoW ne peuvent pas être trackées. Aucun compteur d'overflow n'est exposé dans les logs, aucune alerte n'est émise.

Pour Wayland (v0.3.0+), les compositeurs créent des centaines de buffers partagés. Une saturation silencieuse du CoW tracker sur une session Wayland serait extrêmement difficile à diagnostiquer.

**Action** :
- Ajouter un compteur atomique `COW_OVERFLOW_COUNT` incrémenté à chaque échec d'insertion.
- Exposer ce compteur dans les logs kernel au format `[COW:OVF count=N]` pour détection précoce.
- Documenter la limite dans `docs/kernel/memory/MEMORY_COMPLETE.md`.

---

## Synthèse — Priorités pour v0.2.0

```
Priorité absolue (bloquer la release si non résolus) :
  P0-1  Uniformiser l'ordre de verrouillage dans syscall/mod.rs
  P0-2  Corriger la documentation SYSCALL_TABLE_SIZE
  P0-3  Implémenter SYS_EXO_PROCESS_LIST + corriger cmd_top
  P0-4  Connecter PhoenixPhase réseau à PHOENIX_STATE kernel

À résoudre dans le cycle v0.2.0 :
  P1-1  Marquer loader/ comme non-fonctionnel
  P1-2  Séparer clairement les deux implémentations exosh
  P1-3  Corriger l'ordre SERVICES / known_process_name
  P1-4  Diagnostiquer et fixer la chaîne de calibration TSC
  P1-5  Stripper les binaires embarqués au build
  P1-6  Valider la couverture de tests ExoFS avec un vrai block device

À documenter et planifier :
  P2-1  Retirer aarch64 des architectures officiellement supportées
  P2-2  Supprimer #![allow(static_mut_refs)] progressivement
  P2-3  Clarifier le rôle du serveur virtio_drivers
  P2-4  Déplacer les crates drivers vides hors du workspace principal
  P2-5  Trancher sur la stratégie coreutils vs built-ins exosh
  P2-6  Ajouter un compteur de saturation COW_TABLE
```

---

*— Claude Delta, audit réalisé sur la base du snapshot kernel.zip du 2026-05-13.*
