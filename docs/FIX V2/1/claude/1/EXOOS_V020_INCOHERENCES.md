# ExoOS v0.2.0 — Rapport d'incohérences du kernel

**Rédigé par : Claude Delta**
**Date : 2026-05-15**
**Périmètre : analyse statique complète du dépôt `kernel.zip` (1 334 fichiers)**

---

## Préambule

Ce rapport inventorie toutes les incohérences détectées dans le codebase ExoOS entre
la version actuelle (taguée `0.1.0` dans `Cargo.toml`) et les exigences de la version
`v0.2.0` dite de "stabilisation complète". Les incohérences sont classées par criticité :

- **[BLOQUANT]** : empêche la compilation ou le boot ; doit être résolu avant tout tag v0.2.0
- **[MAJEUR]** : comportement incorrect silencieux à l'exécution
- **[MINEUR]** : dette technique, nommage ou politique incohérente

---

## 1. Crates manquantes dans l'archive — `libs/` absent [BLOQUANT]

**Fichier concerné :** `kernel/Cargo.toml` (lignes 29–30)

```toml
exo-types       = { path = "../libs/exo_types" }
exo-phoenix-ssr = { path = "../libs/exo-phoenix-ssr" }
```

Le répertoire `libs/` n'est pas présent dans l'archive zip livrée. Ces deux crates sont
des dépendances directes du noyau (`kernel/src/lib.rs` importe `exo_os_kernel` qui les
transitif). Sans elles, `cargo build` échoue immédiatement avec des erreurs de résolution
de chemin.

**Action requise :** inclure `libs/exo_types/` et `libs/exo-phoenix-ssr/` dans l'archive
ou les migrer en workspace deps avec path absolu documenté.

---

## 2. Workspace racine absent de l'archive [BLOQUANT]

**Contexte :** plusieurs `Cargo.toml` serveurs utilisent `version.workspace = true`,
`edition.workspace = true`, etc. (exemple : `servers/network_server/Cargo.toml`).

```toml
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
```

Aucun `Cargo.toml` à la racine du zip ne déclare `[workspace]`. Ces champs orphelins
provoqueront une erreur Cargo lors de tout build multi-crate. Le fichier workspace doit
exister à la racine ou les champs doivent être inlinés dans chaque crate.

---

## 3. Version kernel figée à `0.1.0` [MINEUR]

**Fichier :** `kernel/Cargo.toml` ligne 3 : `version = "0.1.0"`

L'objectif de la livraison est la `v0.2.0`. Aucun bump de version n'a été effectué dans
les manifestes Cargo. Cela nuit à la traçabilité des artefacts compilés et aux checks
`semver` éventuels.

**Action requise :** passer `version` à `"0.2.0"` dans tous les `Cargo.toml` concernés
(kernel, loader, exo-boot, servers/\*).

---

## 4. `SmoltcpIface` : `SocketSet` éphémère — pas d'état TCP persistent [MAJEUR]

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

```rust
// Dans poll_one() ET poll_egress() :
let mut sockets = [const { SocketStorage::EMPTY }; SOCKET_STORAGE_LEN];
let mut socket_set = SocketSet::new(&mut sockets[..]);
```

Le `SocketSet` smoltcp est réalloué sur la pile à chaque appel de `poll_one()` et de
`poll_egress()`. Aucun socket TCP/UDP n'est jamais injecté dans ce set avant le poll.
Résultat : smoltcp traite uniquement les paquets L2/L3 en stateless (ARP, IP routing)
mais ne peut maintenir aucun état de connexion TCP (SYN/SYN-ACK, retransmissions,
TIME_WAIT, etc.).

Le `TcpStateStore` maison (`servers/network_server/src/tcp_store.rs`) gère l'état côté
réseau applicatif, mais sans smoltcp pour répondre au niveau TCP wire, les segments
entrants sont silencieusement droppés.

**Impact :** toute connexion TCP initiée par un client externe restera sans réponse.
La pile réseau est fonctionnelle en UDP/ICMP mais non en TCP jusqu'à correction.

**Action requise :** stocker le `SocketSet` (et les sockets smoltcp) comme champs de
`NetworkService`, les hydrater au moment du `poll_*`, ou documenter explicitement que
TCP passera par une autre voie en v0.2.0.

---

## 5. Horloge smoltcp : compteur de ticks, pas de temps réel [MAJEUR]

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

```rust
self.ingress_ticks = self.ingress_ticks.saturating_add(1);
let now = Instant::from_millis(self.ingress_ticks as i64);
```

L'horloge injectée dans smoltcp est un simple compteur d'appels au lieu d'un timestamp
monotonique en millisecondes réelles. Cela rend tous les timers internes à smoltcp
(retransmissions TCP, ARP cache expiry, DHCP leases) incorrects en durée absolue : un
timeout configuré à 200 ms se déclenchera en fait après 200 *appels de poll*, quelle
que soit la charge CPU réelle.

**Action requise :** alimenter `Instant::from_millis()` avec le temps monotonique kernel
via le syscall `clock_gettime(CLOCK_MONOTONIC)` ou via un accès direct à `ktime_get()`.

---

## 6. `DEPS_CRYPTO` manque `ipc_router` [MAJEUR]

**Fichier :** `servers/init_server/src/service_table.rs` ligne 17

```rust
const DEPS_CRYPTO: &[&str] = &["vfs_server"];
```

`crypto_server` dépend de `vfs_server` mais pas de `ipc_router`. Or tout serveur Ring1
communique exclusivement via IPC, dont le registre d'endpoints est géré par `ipc_router`.
Si `ipc_router` n'est pas listé en dépendance, la séquence de démarrage peut théoriquement
tenter de lancer `crypto_server` avant que le bus IPC soit opérationnel (dans un scénario
de relance partielle où seul `vfs_server` est vivant).

**Action requise :** ajouter `"ipc_router"` à `DEPS_CRYPTO`.

---

## 7. `exosh` marqué `critical: true` [MAJEUR]

**Fichier :** `servers/init_server/src/service_table.rs` lignes 130–134

```rust
ServiceMetadata {
    name: "exosh",
    ...
    critical: true,
}
```

Le shell interactif est marqué critique. Par transitivité, `exosh` dépend de `exo_shield`,
qui dépend lui-même de la totalité de la chaîne Ring1 (10 services). Marquer le shell
critique signifie qu'un crash de l'interpréteur de commandes est traité au même niveau
qu'une panne du sous-système IPC ou mémoire. En production bare-metal sans écran de
secours, cela rendrait un crash de shell non-récupérable.

**Action requise :** passer `exosh` en `critical: false`. Un crash du shell doit déclencher
un simple redémarrage supervisé, pas un arrêt système.

---

## 8. `scheduler_server` : `critical: false` mais requis par `exo_shield` [MAJEUR]

**Fichier :** `servers/init_server/src/service_table.rs`

```rust
// scheduler_server : critical: false
// DEPS_EXO_SHIELD inclut "scheduler_server"
// exo_shield : critical: true
```

`exo_shield` est critique et dépend de `scheduler_server`, qui est non-critique.
Si `scheduler_server` ne démarre pas, `exo_shield` ne peut pas se lancer. Mais le
système ne l'interprétera pas comme un échec critique car seul `scheduler_server` est
non-critique. Le résultat est un blocage silencieux de la chaîne de boot sans panique
système.

**Action requise :** aligner la politique : soit `scheduler_server` passe en
`critical: true`, soit il est retiré des dépendances de `exo_shield`.

---

## 9. `loader/` : squelette vide embarqué dans le build [MINEUR]

**Fichiers :** `loader/src/dynamic_linker/` (tous les fichiers)

L'intégralité du dossier `loader/src/dynamic_linker/` ne contient que des types vides
sans implémentation :
- `library.rs` : une struct `LibraryRef` avec un champ
- `resolver.rs` : une enum `ResolveError` à deux variants
- `symbol_table.rs` : une struct `SymbolRef`
- `search_path.rs` : une constante de chemins par défaut
- `version.rs` : une constante `LOADER_ABI_VERSION = 1`

La README du loader confirme que c'est volontaire ("squelette pour phase future"), et
le binaire `_start` sort immédiatement avec `ENOSYS`. Cependant, le module est inclus
dans la compilation sans feature-flag le protégeant, et son `Cargo.toml` ne déclare
aucune feature `dynamic_linking` qui serait cohérente avec le `#[cfg(feature = ...)]`
utilisé dans `main.rs`.

**Action requise :** soit déplacer tout `loader/src/dynamic_linker/` derrière un
`#[cfg(feature = "dynamic_linking")]` cohérent, soit documenter explicitement l'état
"dead code intentionnel" avec `#[allow(dead_code)]` et une note dans le `SKILL.md`.

---

## 10. `arch/aarch64` : `compile_error!()` mais code ARM compilable présent [MINEUR]

**Fichier :** `kernel/src/arch/aarch64/mod.rs`

```rust
#[cfg(target_arch = "aarch64")]
compile_error!("ExoOS v0.2.0 ne supporte pas encore le boot AArch64 ...");

// Mais juste en-dessous :
pub fn read_tsc() -> u64 { /* mrs cntvct_el0 ... */ }
pub fn halt_cpu() -> ! { /* wfi ... */ }
```

Le `compile_error!()` est conditionné à `#[cfg(target_arch = "aarch64")]`, donc il
ne se déclenche que si on compile pour AArch64. Mais les fonctions qui suivent ne sont
pas conditionnées, ce qui signifie qu'elles seront compilées (en version x86_64-void)
et généreront des erreurs d'instructions inconnues si jamais quelqu'un tente un cross
sur une cible ARM64. De plus, `arch/mod.rs` fait `pub use self::aarch64::halt_cpu`
conditionné à `cfg(target_arch = "aarch64")` — mais le `compile_error!()` bloque avant
l'exécution de ce re-export. Logique contradictoire.

**Action requise :** entourer tout le corps de `arch/aarch64/mod.rs` dans
`#[cfg(not(target_arch = "aarch64"))]` ou supprimer le code ARM jusqu'à la phase de
portage officiel.

---

## 11. `#![allow(static_mut_refs)]` global dans `lib.rs` [MAJEUR]

**Fichier :** `kernel/src/lib.rs`

```rust
#![allow(static_mut_refs)]
```

Cette lint (`static_mut_refs`) a été introduite précisément pour signaler les accès
à des références à des mutables statiques susceptibles de créer des data races en
contexte `no_std` multi-cœur. La silencer globalement au niveau du crate masque
potentiellement des UB réels (double-borrow mutable sur des statics partagés entre
CPUs) sans inspection cas par cas.

**Action requise :** retirer le `allow` global et traiter chaque occurrence de
`static mut` avec soit un `Mutex`/`AtomicXxx`, soit un `// SAFETY:` précis justifiant
l'absence de race, selon la règle `regle_bonus.md`.

---

## 12. `#![allow(unexpected_cfgs)]` global — cfgs non déclarés [MINEUR]

**Fichier :** `kernel/src/lib.rs`

```rust
#![allow(unexpected_cfgs)]
```

Les cfgs `exo_boot_payloads` et `exo_kernel_trace` utilisés dans `userspace_boot.rs`
ne sont déclarés dans aucun `[features]` ni `[package.metadata]` du `Cargo.toml`.
Le `allow(unexpected_cfgs)` est une rustup de contournement plutôt qu'une déclaration
propre. Les builds reproductibles et les outils d'analyse statique (clippy, cargo-deny)
en souffrent.

**Action requise :** déclarer ces cfgs dans `kernel/Cargo.toml` :

```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(exo_boot_payloads)', 'cfg(exo_kernel_trace)'] }
```

ou les migrer en `[features]`.

---

## 13. Hook NVMe nommé `register_nvme_flush_fn` pour un backend VirtIO-Blk [MINEUR]

**Fichiers :** `kernel/src/fs/exofs/epoch/epoch_barriers.rs` et `kernel/src/fs/exofs/mod.rs`

```rust
// Dans epoch_barriers.rs :
static FLUSH_HOOK: Mutex<NvmeFlushFn> = ...;
pub fn register_nvme_flush_fn(flush_fn: NvmeFlushFn) { ... }

// Dans mod.rs, à l'init :
fn register_storage_flush_barrier() {
    crate::fs::exofs::epoch::epoch_barriers::register_nvme_flush_fn(
        crate::fs::exofs::storage::virtio_adapter::flush_global_disk, // <- VirtIO !
    );
}
```

Le hook de flush s'appelle `nvme_flush_fn` mais est câblé sur `virtio_adapter::flush_global_disk`.
En v0.2.0, le backend block de boot est exclusivement VirtIO-Blk — il n'y a pas de
contrôleur NVMe. Ce nommage trompeur peut induire en erreur lors des futurs travaux sur
le vrai driver NVMe (AHCI/NVMe qui existe dans `drivers/storage/`).

**Action requise :** renommer en `register_block_flush_fn` / `FLUSH_HOOK: BlockFlushFn`
et adapter les appellants.

---

## 14. Preuves d'exécution des tests ExoFS tier_4 et tier_6 absentes [BLOQUANT pour v0.2.0]

**Fichier :** `kernel/src/fs/exofs/tests/TESTS_STATUS_REPORT.md`

Le rapport de tests lui-même déclare explicitement :

> *"Couverture présente dans le code, mais preuve d'exécution non inscrite ici. Un
> run CI/WSL doit attacher ses logs avant de déclarer ExoFS totalement valide."*

Les tiers critiques pour v0.2.0 sont :
- `tier_4_pipeline` — backend VFS réel requis
- `tier_6_virtio_vfs` — chemin VirtIO/VFS requis

Ces deux tiers n'ont aucun log de passage enregistré. Pour un objectif de
"stabilisation complète", l'absence de preuve d'exécution de ces tests constitue un
critère de blocage formel.

**Action requise :** exécuter en WSL ou CI :

```bash
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
    fs::exofs::tests::integration::tier_4_pipeline
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu \
    fs::exofs::tests::integration::tier_6_virtio_vfs
```

et attacher les logs au commit de tag v0.2.0.

---

## 15. `servers/phase5-tests/rustc-ice-*.txt` — ICE compilateur non résolu dans le dépôt [MINEUR]

**Fichier :** `servers/phase5-tests/rustc-ice-2026-04-18T23_43_24-667.txt`

Un fichier de crash compilateur Rust (Internal Compiler Error, ICE) daté du 18 avril 2026
est versionné dans le dépôt. Le crash se produit dans `annotate_snippets` lors du passage
`check_mod_deathness` (analyse des symboles morts). Ce bug est probablement déclenché par
un pattern de code spécifique dans `servers/phase5-tests/src/lib.rs` (48 519 lignes).

Deux problèmes :
1. Un ICE non résolu dans le code des tests indique un code qui ne compilait pas proprement.
2. Ce fichier n'a aucune place dans un dépôt de production versionné.

**Action requise :** déplacer le fichier dans un wiki ou issue tracker externe, et vérifier
si le code de `phase5-tests` compile sans ICE avec la toolchain courante.

---

## 16. `exofs/mod.rs` : `exofs_register_fs()` non appelée dans la séquence de boot [MAJEUR]

**Fichier :** `kernel/src/fs/exofs/mod.rs`

```rust
pub fn exofs_init(disk_size_bytes: u64) -> Result<(), ExofsError> {
    // ...
    // Phase 2 : Enregistrement VFS
    // Omis ici : enregistrement effectué après le boot via exofs_register_fs().
    // ...
}
```

`exofs_init()` délègue explicitement l'enregistrement VFS à `exofs_register_fs()`, et
précise que c'est appelé "après le boot". Mais aucune trace d'appel à `exofs_register_fs()`
n'est présente dans `kernel/src/main.rs`, `userspace_boot.rs`, ni dans la séquence
`arch_boot_init()` / `kernel_init()`. Sans cet appel, ExoFS est initialisé mais les
opérations VFS POSIX (`open`, `read`, `write`, etc.) sur les chemins ExoFS restent
non enregistrées dans la dispatch table VFS.

**Action requise :** identifier le point d'appel prévu et s'assurer qu'il est bien
effectif dans la séquence de boot documentée, ou l'ajouter dans `kernel_init()`.

---

## 17. `EXO_SHIELD_BIN` : nom incohérent entre `service_table` et `userspace_boot` [MINEUR]

**Fichier :** `servers/init_server/src/service_table.rs` vs `kernel/src/userspace_boot.rs`

```rust
// service_table.rs :
pub static EXO_SHIELD_BIN: &[u8] = b"/sbin/exo-shield\0";  // exo-shield

// userspace_boot.rs :
EmbeddedPayload { path: "/sbin/exo-shield", bytes: &EXO_SHIELD_BYTES }  // cohérent

// MAIS le service dans SERVICES[] s'appelle "exo_shield" (underscore)
// et le nom IPC attendu dans boot_sequence.rs est "exo_shield" (underscore)
```

L'IPC lookup dans `boot_sequence.rs` utilise le nom de service `"exo_shield"` (avec
underscore), mais le binaire s'appelle `exo-shield` (avec tiret). Si le service s'enregistre
lui-même sous `"exo-shield"` (tiret) côté IPC au lieu de `"exo_shield"` (underscore), la
barrière `endpoint_registered("exo_shield")` ne sera jamais satisfaite et le service sera
considéré en timeout puis tué.

**Action requise :** vérifier que le nom IPC auto-enregistré dans
`servers/exo_shield/src/main.rs` correspond exactement au nom attendu dans
`service_table.rs` (`"exo_shield"` avec underscore).

---

## Résumé des priorités v0.2.0

| # | Catégorie | Description courte | Criticité |
|---|-----------|-------------------|-----------|
| 1 | Build | `libs/exo_types` et `exo-phoenix-ssr` absents | **BLOQUANT** |
| 2 | Build | Workspace racine manquant | **BLOQUANT** |
| 14 | Tests | Logs tier_4 et tier_6 ExoFS absents | **BLOQUANT** |
| 4 | Réseau | `SocketSet` smoltcp éphémère — TCP non persistant | **MAJEUR** |
| 5 | Réseau | Horloge smoltcp non réelle | **MAJEUR** |
| 6 | Init | `DEPS_CRYPTO` manque `ipc_router` | **MAJEUR** |
| 7 | Init | `exosh` critique à tort | **MAJEUR** |
| 8 | Init | `scheduler_server` non-critique mais requis par `exo_shield` | **MAJEUR** |
| 11 | Sécurité | `allow(static_mut_refs)` global | **MAJEUR** |
| 16 | FS | `exofs_register_fs()` jamais appelée au boot | **MAJEUR** |
| 3 | Packaging | Version Cargo figée à 0.1.0 | **MINEUR** |
| 9 | Code | Loader squelette sans feature-flag | **MINEUR** |
| 10 | Arch | aarch64 compile_error + code ARM contradictoires | **MINEUR** |
| 12 | Build | `unexpected_cfgs` silencé globalement | **MINEUR** |
| 13 | FS | Nommage `nvme_flush` sur backend VirtIO | **MINEUR** |
| 15 | Dev | ICE compilateur versionné dans le dépôt | **MINEUR** |
| 17 | Init | Nom IPC `exo_shield` vs `exo-shield` potentiellement incohérent | **MINEUR** |

---

*Rapport généré par analyse statique complète. Aucune exécution de code n'a été effectuée.
Les points BLOQUANTS doivent être résolus avant tout tag `v0.2.0`.*

*— Claude Delta, 2026-05-15*
