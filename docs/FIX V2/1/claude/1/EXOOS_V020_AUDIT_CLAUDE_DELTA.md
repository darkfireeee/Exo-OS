# Audit de Stabilisation — Exo-OS Kernel v0.2.0
**Rédigé par : Claude Delta**
**Date : 15 mai 2026**
**Version auditée : post-v0.1.0 → cible v0.2.0**

---

## Préambule

Ce document recense toutes les incohérences identifiées dans le kernel Exo-OS
après lecture exhaustive du dépôt `kernel.zip`. L'objectif de v0.2.0 étant une
**stabilisation complète**, chaque point ci-dessous constitue un bloquant ou un
risque à solder avant de passer à la phase Wayland/installation visuelle.

Les findings sont classés par sévérité : **BLOQUANT**, **MAJEUR**, **MINEUR**,
**DOC**.

---

## BLOQUANT — B01 : `libs/` absente du dépôt (build impossible)

**Fichier concerné :** `kernel/Cargo.toml`

```toml
[dependencies]
exo-types       = { path = "../libs/exo_types" }
exo-phoenix-ssr = { path = "../libs/exo-phoenix-ssr" }
```

Le répertoire `libs/` n'existe pas dans l'archive. Ces deux crates sont
référencés par chemin relatif mais sont **introuvables**. Le kernel ne compile
pas en l'état.

`exo-types` exporte `IpcEndpoint`, utilisé dans `kernel/src/ipc/mod.rs` :

```rust
use exo_types::IpcEndpoint;
// ...
pub fn send_irq_notification(endpoint: &IpcEndpoint, ...) -> Result<(), IpcError>
```

`exo-phoenix-ssr` est probablement utilisé dans `exophoenix/ssr.rs`.

**Action requise :** Créer `libs/exo_types/` et `libs/exo-phoenix-ssr/` avec
leurs `Cargo.toml` et sources, ou déplacer les types directement dans
`kernel/src/` et supprimer les dépendances externes.

---

## BLOQUANT — B02 : Absence de workspace `Cargo.toml` racine

**Fichiers concernés :** tous les `servers/*/Cargo.toml`

Chaque serveur utilise les champs workspace :

```toml
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
spin.workspace = true
```

Il n'existe **aucun `Cargo.toml` à la racine du projet** déclarant le
workspace. Sans ce fichier, `cargo build` dans n'importe quel serveur échoue
avec `error: failed to read workspace root` ou résout les champs `workspace`
à vide.

**Action requise :** Créer `/Cargo.toml` avec une section `[workspace]`
listant tous les membres (`kernel`, `servers/*`, `drivers/*`, etc.) et une
section `[workspace.dependencies]` pour les dépendances partagées (`spin`,
`bitflags`, `blake3`, etc.).

---

## MAJEUR — M01 : `cgroup::init()` documenté mais jamais appelé

**Fichier concerné :** `kernel/src/lib.rs` — `kernel_init()`, Phase 4

Le commentaire de `process/mod.rs` décrit explicitement 6 étapes d'init, dont :

> 6. resource::cgroup::init() — initialise le cgroup racine

Le commentaire dans `kernel_init()` le rappelle aussi :

```rust
// NOTE: process::init() appelle cgroup::init() qui référence CGROUP_TABLE (~28KB .data).
```

Mais le code de la Phase 4 ne contient **aucun appel** à
`crate::process::resource::cgroup::init()` :

```rust
crate::process::core::pid::init(32768, 131072);
crate::process::core::registry::init(32768);
crate::process::lifecycle::reap::init_reaper();
crate::process::state::wakeup::register_with_dma();
crate::memory::register_oom_kill_sender(process_oom_kill_sender);
// ← cgroup::init() manquant ici
```

La `CGROUP_TABLE` est donc dans un état non initialisé au moment où des
processus pourraient tenter de l'utiliser.

**Action requise :** Ajouter `crate::process::resource::cgroup::init();` dans
la Phase 4 de `kernel_init()`, après `register_oom_kill_sender`.

---

## MAJEUR — M02 : `memory::cow::init()` non appelé dans `kernel_init()`

**Fichier concerné :** `kernel/src/memory/cow/mod.rs`, `kernel/src/lib.rs`

Le module `memory/cow/mod.rs` expose :

```rust
pub fn init() {
    let _ = COW_TRACKER
        .tracked_count
        .load(core::sync::atomic::Ordering::Relaxed);
}
```

Ce hook documente et « fige l'ordre d'initialisation après SLUB, avant les
sous-systèmes qui peuvent créer des mappings CoW ». Or `kernel_init()` ne
contient **aucun appel** à `crate::memory::cow::init()`. Les mappings CoW
(utilisés par `fork()`) peuvent donc s'activer avant l'établissement de l'ordre
garanti.

**Action requise :** Appeler `crate::memory::cow::init()` dans `kernel_init()`
Phase 2 (après SLUB, avant le scheduler), conformément au commentaire du module.

---

## MAJEUR — M03 : `sleep_timer_wake` — risque use-after-free sur le TCB

**Fichier concerné :** `kernel/src/scheduler/timer/sleep.rs`

La callback du timer stocke le pointeur brut du TCB comme `u64` :

```rust
let timer_id = hrtimer::arm(cpu_raw, delay_ns, tcb as u64, sleep_timer_wake);
```

```rust
unsafe fn sleep_timer_wake(_id: u32, data: u64) {
    let tcb = data as *mut ThreadControlBlock;
    let Some(tcb_nn) = NonNull::new(tcb) else { return; };
    let tcb_ref = &*tcb_nn.as_ptr();
    if !tcb_ref.try_transition(TaskState::Sleeping, TaskState::Runnable) {
        return;
    }
    // ...enqueue
}
```

Si le thread se termine (et que son TCB est libéré) **avant** que le timer
n'expire, `sleep_timer_wake` déréférence un pointeur dangling. Il n'existe
aucune protection visible (pas de flag `tcb_alive`, pas de refcount sur le TCB,
pas d'annulation du timer dans le chemin `do_exit`).

**Action requise :** Soit annuler le hrtimer dans `do_exit`/`do_exit_thread`
avant de libérer le TCB, soit ajouter un refcount ou un flag atomique
`timer_pending` vérifié dans `sleep_timer_wake` après `NonNull::new`.

---

## MAJEUR — M04 : `phase5-tests` — ICE rustc committé, crate isolé

**Fichier concerné :** `servers/phase5-tests/rustc-ice-2026-04-18T23_43_24-667.txt`

Un crash interne du compilateur (`rustc` ICE) a été committé dans le dépôt.
L'erreur :

```
slice index starts at 9 but ends at 8
```

indique un bug dans `annotate_snippets` déclenché lors de la compilation de
`phase5-tests`. Ce crate possède de plus son propre `Cargo.lock` (inhabituel
dans un workspace), ce qui confirme qu'il est isolé et non intégré au reste du
build.

**Action requise :**
- Reproduire et documenter le cas qui déclenche l'ICE.
- Soit mettre à jour la chaîne toolchain (`rust-toolchain.toml`) vers une
  version corrigée, soit simplifier le code responsable.
- Supprimer le fichier ICE du dépôt (il ne doit pas être tracké).
- Intégrer `phase5-tests` dans le workspace racine avec un `Cargo.lock` partagé.

---

## MAJEUR — M05 : Transition Phoenix `Draining → Serialized` sans fenêtre observable

**Fichier concerné :** `servers/network_server/src/isolation.rs`

La méthode `prepare()` fait :

```rust
self.phase = PhoenixPhase::Draining;
Self::sync_kernel_phase(self.phase);   // syscall kernel
iface.drain_all(device, pool);         // synchrone
driver.flush_released(device);
store.clear();
self.phase = PhoenixPhase::Serialized;
Self::sync_kernel_phase(self.phase);
```

L'état `Draining` est notifié au kernel, puis toutes les opérations de drain
sont **synchrones et non interruptibles**. Tout observateur externe (sentinel,
watchdog) qui lirait `PHOENIX_STATE` après le premier `sync_kernel_phase` mais
voudrait réagir à `Draining` n'a aucune garantie temporelle sur la durée de
cet état. Si `drain_all` est long, le watchdog peut déclencher un timeout.

De plus, si `drain_all` ou `flush_released` panique, l'état kernel reste
`NetworkDraining` indéfiniment sans retour à `Normal`.

**Action requise :**
- Ajouter un timeout explicite autour de `drain_all` dans `prepare()`.
- En cas d'échec/panique dans `prepare()`, appeler `restore()` dans le
  `panic_handler` ou via `Drop` pour remettre l'état kernel à `Normal`.
- Documenter la durée maximale tolérée pour la phase `Draining` dans l'audit
  EXONET_V4.

---

## MINEUR — m01 : `aarch64/mod.rs` — `compile_error!` avant des fonctions actives

**Fichier concerné :** `kernel/src/arch/aarch64/mod.rs`

```rust
#[cfg(target_arch = "aarch64")]
compile_error!(
    "ExoOS v0.2.0 ne supporte pas encore le boot AArch64; ..."
);

// ...puis des fonctions read_tsc(), halt_cpu(), irq_save(), etc.
```

Le `compile_error!` bloque la compilation pour `aarch64`, mais les fonctions
en dessous sont quand même parsées et potentiellement compilées dans d'autres
contextes. C'est ambigu pour un lecteur : le module *semble* fonctionnel alors
qu'il est volontairement non supporté.

**Action requise :** Envelopper tout le contenu après le `compile_error!` dans
`#[cfg(not(target_arch = "aarch64"))]` ou déplacer les stubs placeholder dans
un sous-fichier `stubs.rs` explicitement commenté comme « futurs ».

---

## MINEUR — m02 : `stage0.rs` — numérotation des étapes incohérente

**Fichier concerné :** `kernel/src/exophoenix/stage0.rs`

Le commentaire d'en-tête liste :

```
// 1) install page tables B
// 0.5) probe CPUID global   ← step 0.5 APRÈS step 1
// 2) stack B + guard page
// ...
```

L'étape `0.5` est documentée après l'étape `1`, ce qui n'a pas de sens dans
un ordre séquentiel.

**Action requise :** Renuméroter les étapes dans l'ordre logique (0, 0.5, 1,
2, …) ou supprimer le 0.5 et renommer en `0b` / `1a`.

---

## MINEUR — m03 : `exosh` marqué `critical: true` — risque de boot bloqué

**Fichier concerné :** `servers/init_server/src/service_table.rs`

```rust
ServiceMetadata {
    name: "exosh",
    requires: DEPS_EXOSH,  // ["tty_server", "vfs_server", "exo_shield"]
    ready_timeout_ms: 500,
    critical: true,
}
```

`exosh` est le shell interactif. Le marquer `critical: true` signifie qu'un
échec de démarrage du shell **bloque le boot** entier. En pratique pour v0.2.0
(cible QEMU), si `exo_shield` dépasse son timeout de 1000ms (lui aussi
`critical: true`), `exosh` ne peut jamais démarrer. Deux services critiques en
cascade créent un risque de boot mort sans diagnostic visible.

**Action requise :** Passer `exosh` en `critical: false` pour v0.2.0. Le shell
est important pour l'interaction mais son absence ne doit pas empêcher les
services système de continuer à tourner.

---

## MINEUR — m04 : `DEPS_EXOSH` — double dépendance implicite sur `vfs_server`

**Fichier concerné :** `servers/init_server/src/service_table.rs`

```rust
const DEPS_EXOSH:     &[&str] = &["tty_server", "vfs_server", "exo_shield"];
const DEPS_EXO_SHIELD: &[&str] = &["ipc_router", "memory_server", "vfs_server", ...];
```

`exosh` dépend directement de `vfs_server` ET de `exo_shield` qui dépend aussi
de `vfs_server`. Cette redondance n'est pas un bug, mais si l'ordre de
résolution des dépendances est plat (non-topologique), cela peut générer une
double tentative de démarrage de `vfs_server`.

**Action requise :** Vérifier que le resolver de dépendances dans
`init_server/src/boot_sequence.rs` déduplique les entrées avant lancement.

---

## DOC — D01 : Commentaire orphelin sur `cgroup::init()` dans `kernel_init`

**Fichier concerné :** `kernel/src/lib.rs`, Phase 4

```rust
// NOTE: process::init() appelle cgroup::init() qui référence CGROUP_TABLE (~28KB .data).
```

Ce commentaire est faux (voir M01) : `cgroup::init()` n'est pas appelé. Même
si M01 est corrigé, le commentaire dit « process::init() appelle » alors que
l'appel est **direct** dans `kernel_init()`, pas via une fonction `process::init()`.

**Action requise :** Après correction de M01, mettre à jour le commentaire pour
refléter l'appel direct `crate::process::resource::cgroup::init()`.

---

## DOC — D02 : `loader/README.md` — confusion sur le rôle du loader en v0.2.0

**Fichier concerné :** `loader/README.md`

> En v0.2.0, le boot et `execve` utilisent le loader ELF statique du kernel
> (`kernel/src/fs/elf_loader_impl.rs`) et les payloads Ring1 sont des binaires
> statiques embarqués.

Ce README est correct mais la phrase « le binaire bare-metal sort immédiatement
avec `ENOSYS` tant que la feature `dynamic_linking` n'est pas activée » implique
que le loader peut être lancé accidentellement. Or, `exosh` et d'autres composants
pourraient tenter de l'invoquer si un chemin de résolution ELF choisit le loader
dynamique. Il manque un test de non-régression qui vérifie que `loader` n'est
jamais invoqué en v0.2.0.

**Action requise :** Ajouter un test d'intégration ou une assertion dans
`boot_sequence.rs` qui log une erreur si le loader dynamique est accidentellement
résolu pendant la phase de boot.

---

## DOC — D03 : `TESTS_STATUS_REPORT.md` — validation ExoFS bloquée sans CI

**Fichier concerné :** `kernel/src/fs/exofs/tests/TESTS_STATUS_REPORT.md`

Le rapport indique explicitement :

> Un run CI/WSL doit attacher ses logs avant de déclarer ExoFS totalement valide.

Pour une version v0.2.0 de stabilisation, l'absence d'exécution CI certifiée
des tiers critiques (tier_4_pipeline, tier_6_virtio_vfs) est **incompatible**
avec le label « stabilisation complète ».

**Action requise :** Exécuter et attacher les logs des tests suivants dans le
dépôt avant de taguer v0.2.0 :
```
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
    fs::exofs::tests::integration::tier_4_pipeline
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
    fs::exofs::tests::integration::tier_6_virtio_vfs
```

---

## Récapitulatif

| ID  | Sévérité  | Titre court                                          | Fichier principal                              |
|-----|-----------|------------------------------------------------------|------------------------------------------------|
| B01 | BLOQUANT  | Dossier `libs/` manquant (exo-types, phoenix-ssr)    | `kernel/Cargo.toml`                            |
| B02 | BLOQUANT  | Workspace `Cargo.toml` racine absent                 | `/` (racine projet)                            |
| M01 | MAJEUR    | `cgroup::init()` jamais appelé                       | `kernel/src/lib.rs`                            |
| M02 | MAJEUR    | `cow::init()` jamais appelé                          | `kernel/src/lib.rs`                            |
| M03 | MAJEUR    | Use-after-free potentiel dans `sleep_timer_wake`     | `kernel/src/scheduler/timer/sleep.rs`          |
| M04 | MAJEUR    | ICE rustc committé, `phase5-tests` isolé             | `servers/phase5-tests/`                        |
| M05 | MAJEUR    | Transition Phoenix Draining sans fenêtre robuste     | `servers/network_server/src/isolation.rs`      |
| m01 | MINEUR    | `aarch64/mod.rs` — compile_error avant fonctions     | `kernel/src/arch/aarch64/mod.rs`               |
| m02 | MINEUR    | `stage0.rs` — numérotation étapes incohérente        | `kernel/src/exophoenix/stage0.rs`              |
| m03 | MINEUR    | `exosh` marqué critical — boot bloquable             | `servers/init_server/src/service_table.rs`     |
| m04 | MINEUR    | Double dépendance `vfs_server` via exo_shield/exosh  | `servers/init_server/src/service_table.rs`     |
| D01 | DOC       | Commentaire orphelin cgroup dans kernel_init         | `kernel/src/lib.rs`                            |
| D02 | DOC       | README loader — absence de test non-régression       | `loader/README.md`                             |
| D03 | DOC       | ExoFS — validation tier_4/tier_6 sans CI             | `kernel/src/fs/exofs/tests/TESTS_STATUS_REPORT.md` |

---

## Ordre de résolution recommandé pour v0.2.0

1. **B01** + **B02** d'abord — sans ça, rien ne compile.
2. **M01** + **M02** — correctifs d'une ligne chacun, risque élevé.
3. **M03** — sécurité mémoire, critique avant tests de stress scheduler.
4. **M04** — nettoyer le crate phase5-tests et son ICE.
5. **M05** — fiabilité réseau avant la phase Wayland (qui dépend du stack réseau).
6. **m01**–**m04** + **D01**–**D03** — nettoyage documentation et mineurs.

---

*— Claude Delta, audit kernel ExoOS v0.2.0, 15 mai 2026*
