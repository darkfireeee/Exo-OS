# ExoOS — Audit des Incohérences Kernel v0.2.0 — Snapshot 2026-05-20
## Rapport de stabilisation — Itération 2

**Rédigé par** : Claude Delta  
**Date** : 2026-05-20  
**Base** : ExoOS — kernel.zip snapshot 2026-05-20 (post-résolution partielle CORR-75 à CORR-86)  
**Référence** : `docs/Vision v0.2.0/VISION-V0.2.0.md`, `MASTER-CHECKLIST-V0.2-REV2.md`, `MASTER-CORRECTIONS-V0.2.md`  
**Précédent rapport** : `docs/FIX V2/1/claude/EXOOS_v0.2.0_AUDIT_INCOHERENCES_claude_delta.md` (2026-05-14)

---

## Préambule — Ce qui a changé depuis l'itération 1

Trois corrections P0 du rapport précédent ont été **appliquées avec succès** :

| ID | Description | Verdict |
|----|-------------|---------|
| P0-1 | Ordre de verrouillage inversé dans `syscall/mod.rs` | ✅ **RÉSOLU** — libellé corrigé, conforme à `ipc/mod.rs` |
| P0-3 | `cmd_top` : sondage PID par `kill(0)` + table statique | ✅ **RÉSOLU** — `SYS_EXO_PROCESS_LIST = 351` implémenté, `cmd_top()` l'utilise |
| P0-4 | `PhoenixPhase` réseau déconnecté de `PHOENIX_STATE` kernel | ✅ **RÉSOLU** — `network_server/src/isolation.rs` appelle `SYS_EXO_PHOENIX_STATE_SET` |

Ce rapport couvre les **incohérences nouvelles ou persistantes** du snapshot 2026-05-20, vérifiées dans le code source réel.

---

## Sommaire des gravités

| Gravité | Nombre | Impact |
|---------|--------|--------|
| **P0 — Bloquant** | 3 | Corrections CORR-84, CORR-86, CORR-75-A non appliquées : persistance brisée, sécurité absente, ExoShield fantôme |
| **P1 — Majeur** | 5 | Incohérences techniques significatives, blocage partiel de BLOC 0 et BLOC 2 |
| **P2 — Mineur** | 5 | Dettes techniques portées de l'itération précédente, à documenter |

---

## P0 — Incohérences Bloquantes

### P0-1 · `DEFAULT_VIRTIO_BLK_MMIO_BASE = 0x1000_0000` hardcodé en production

**Fichiers concernés** :
- `kernel/src/fs/exofs/storage/virtio_adapter.rs:9–10` ← **production**
- `drivers/storage/virtio_blk/src/lib.rs:86,94,110` ← tests (secondaire)

**Constat** :

La constante de production est déclarée ainsi :

```rust
// kernel/src/fs/exofs/storage/virtio_adapter.rs
pub const DEFAULT_VIRTIO_BLK_MMIO_BASE: usize = 0x1000_0000;
pub const DEFAULT_VIRTIO_BLK_CAPACITY_BYTES: usize = 512 * 1024 * 1024;
```

Et `init_global_disk()` — le chemin de boot nominal — l'utilise directement :

```rust
pub fn init_global_disk() {
    init_global_disk_with_mmio(
        DEFAULT_VIRTIO_BLK_MMIO_BASE,   // ← 0x1000_0000 hardcodé
        DEFAULT_VIRTIO_BLK_CAPACITY_BYTES,
    );
}
```

L'adresse `0x1000_0000` (256 MiB) est dans la plage de la RAM physique avec `-m 256M`. Le BAR0 réel de `virtio-blk-pci` dans QEMU est autour de `0xC000_0000`, ce qui diffère entièrement. En conséquence, **ExoFS ne lit ni n'écrit jamais sur le disque** — toutes les données restent en RAM et sont perdues au reboot. La CORR-86 qui devait corriger cela n'a pas été appliquée.

**Impact** : `exo compat install calendar` "réussit" en mémoire mais l'installation disparaît au redémarrage. Le critère `B-02` (ExoFS persiste sur disque après reboot) est structurellement impossible sans ce fix. Bloque BLOC -1 entier.

**Correction** : Lire le BAR0 depuis le PCI config space au lieu d'utiliser une constante fixe. La séquence :

```rust
// Lire BAR0 depuis PCI config space (adresse 0x10 dans la PCI header)
let bar0 = pci_config_read32(bus, device, func, PCI_BAR0_OFFSET);
let mmio_base = (bar0 & !0xF) as usize; // masquer les bits de type
init_global_disk_with_mmio(mmio_base, detected_capacity);
```

---

### P0-2 · `is_immutable()` jamais vérifié dans le chemin d'écriture ExoFS

**Fichiers concernés** :
- `kernel/src/fs/exofs/syscall/object_write.rs` — chemin principal d'écriture
- `kernel/src/fs/exofs/objects/object_meta.rs:100,417` — flag défini mais inutilisé dans write

**Constat** :

Le flag `META_FLAG_IMMUTABLE` existe et est correctement défini :

```rust
// object_meta.rs
pub const META_FLAG_IMMUTABLE: u32 = 1 << 0;

pub fn is_immutable(&self) -> bool {
    self.extra_flags & META_FLAG_IMMUTABLE != 0
}
```

Mais la fonction `write_blob()` dans `object_write.rs` appelle directement `BLOB_CACHE.write_at()` **sans jamais consulter ce flag** :

```rust
fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    // ... validation offset ...
    ensure_blob_cached(blob_id)?;
    // ← AUCUN APPEL À is_immutable() ICI
    BLOB_CACHE.write_at(blob_id, start, data)?;  // écriture directe
    persist_cached_blob_if_disk(blob_id)?;
    // ...
}
```

ExoLedger est marqué comme objet immutable dans ExoFS. Un processus Ring3 disposant de la capability `ExoFsObjectWrite` sur le blob ExoLedger peut **modifier les entrées d'audit** via `SYS_EXOFS_OBJECT_WRITE` sans déclencher la moindre erreur. La CORR-84 n'a pas été appliquée.

**Impact** : ExoLedger n'est pas immutable en pratique. L'invariant de sécurité `S19` (critère de checklist BLOC 2) est brisé. Tout audit forensique est compromettable.

**Correction** (telle que décrite dans CORR-84) :

```rust
fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    // Vérifier immutabilité avant toute écriture
    if let Ok(meta) = blob_meta_cache_get(blob_id) {
        if meta.is_immutable() {
            exoledger_append(current_pid(), LedgerEvent::WriteOnImmutable { blob_id });
            return Err(ExofsError::AccessDenied(AccessDeniedReason::Immutable));
        }
    }
    // ... suite inchangée ...
}
```

---

### P0-3 · `exo_shield/src/lib.rs` : 5 modules sur 10 sont des fantômes

**Fichier concerné** : `servers/exo_shield/src/lib.rs`

**Constat** :

Le fichier `lib.rs` d'`exo_shield` ne déclare que 4 modules :

```rust
// servers/exo_shield/src/lib.rs — état actuel
#![no_std]

pub mod behavioral;
pub mod engine;
pub mod ipc_gate;
pub mod signatures;
```

Or les répertoires physiques suivants existent dans le dépôt avec leur code source :

```
servers/exo_shield/src/hooks/         (exec_hooks, memory_hooks, net_hooks, syscall_hooks)
servers/exo_shield/src/sandbox/       (container, fs_restriction, net_isolation, syscall_filter)
servers/exo_shield/src/network/       (dns_guard, firewall, ids, traffic_analysis)
servers/exo_shield/src/ml/            (features, inference, model, update)
servers/exo_shield/src/forensics/     (memory_dump, report, timeline)
```

Ces 5 modules **compilent mais ne sont jamais chargés** — ils sont absents de `lib.rs`. En conséquence :
- Les hooks syscall (`syscall_hooks.rs`) ne s'attachent à aucun chemin d'exécution.
- Le moteur de containment sandbox (`container.rs`) n'est jamais instancié.
- Le firewall réseau (`firewall.rs`) n'inspecte aucun paquet.
- Le moteur ML d'inférence comportementale (`inference.rs`) n'est pas initialisé.
- Le module forensics (`memory_dump.rs`) ne peut pas être déclenché.

La CORR-75-A (ajout des 5 modules à `lib.rs`) et les CORR-75-B à 75-F (initialisation dans `_start()`, branchement dans `handle_event_report()`, etc.) n'ont pas été appliquées.

**Impact** : ExoShield est un coquille vide. Les critères `ES-01` à `ES-06` de BLOC 11 sont à 0%. La chaîne de sécurité décrite dans `VISION-V0.2.0.md §3.2` s'arrête au moteur de signatures.

**Correction** : Ajouter à `lib.rs` :

```rust
pub mod hooks;
pub mod sandbox;
pub mod network;
pub mod ml;
pub mod forensics;
```

Puis initialiser chaque module dans `main.rs:_start()` comme décrit dans CORR-75-B.

---

## P1 — Incohérences Majeures

### P1-1 · `syscall/numbers.rs` : plage `[400..499]` déclarée réservée mais occupée par des syscalls POSIX réels

**Fichier concerné** : `kernel/src/syscall/numbers.rs:8`

**Constat** :

La documentation en tête du fichier indique :

```
//! - [400..499] : réservés pour usage futur
```

Or les constantes suivantes sont définies dans cette même plage :

```rust
pub const SYS_OPENAT2: u64 = 437;        // syscall Linux standard
pub const SYS_EPOLL_PWAIT2: u64 = 441;   // syscall Linux standard
```

Ces deux syscalls POSIX (`openat2` introduit dans Linux 5.6, `epoll_pwait2` dans Linux 5.11) sont des extensions modernes de la compatibilité POSIX. Ils sont implémentés dans ExoOS — `SYS_OPENAT2 = 437` correspond précisément au numéro Linux x86_64 officiel.

Le rapport d'audit précédent (P0-2, itération 1) avait signalé une incohérence similaire concernant la borne `512`. Cette correction a été appliquée **partiellement** : la borne 512 et le bloc ExoFS ont été documentés correctement, mais la plage `[400..499]` a été déclarée réservée sans tenir compte de ces deux entrées réelles.

**Impact** : Tout générateur de stubs libc (musl-exo, tests ABI) qui respecte la documentation produira une table qui traite 437 et 441 comme des trous `ENOSYS`. Incohérence de documentation qui deviendra un bug de runtime au moment où musl-exo sera intégré.

**Correction** :

```
//! - [400..436] : réservés pour usage futur
//! - [437]      : SYS_OPENAT2 (Linux compat)
//! - [438..440] : réservés pour usage futur
//! - [441]      : SYS_EPOLL_PWAIT2 (Linux compat)
//! - [442..499] : réservés pour usage futur
```

---

### P1-2 · `IPC_FLAG_INJECT_SRC_PID` : flag accessible à tout processus Ring3 sans capability

**Fichier concerné** : `kernel/src/syscall/table.rs:2726–2732`

**Constat** :

Le flag `IPC_FLAG_INJECT_SRC_PID = 0x0002` peut être passé par n'importe quel appelant de `sys_exo_ipc_send()`. Quand il est positionné, le kernel écrase `payload[0..4]` avec le PID réel de l'appelant :

```rust
// syscall/table.rs:2726
if flags & IPC_FLAG_INJECT_SRC_PID != 0 {
    if len < core::mem::size_of::<u32>() {
        return EINVAL;
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    // ← AUCUNE VÉRIFICATION DE CAPABILITY OU DE RING
}
```

Il n'y a aucune vérification que l'appelant est Ring1 ou dispose d'une capability spécifique. Un processus Ring3 malveillant peut :
1. Envoyer un message `IPC_FLAG_INJECT_SRC_PID` à un serveur Ring1.
2. Le serveur recevra `payload[0..4]` = PID Ring3, **injecté par le kernel lui-même**.
3. Si le serveur Ring1 lit `payload[0..4]` comme "PID source authoritativement fourni par le kernel", il accorde une confiance indue à cet identifiant.

L'analyse de `servers/exo_shield/src/ipc_gate/access.rs` montre que `payload[0..4]` est effectivement utilisé comme identifiant dans certains cas. Le vecteur d'abus n'est pas trivial mais la surface d'attaque existe.

**Impact** : Violation du principe zero-trust. Un processus Ring3 ne devrait pas pouvoir forcer le kernel à "certifier" son PID dans un payload IPC sans capability dédiée.

**Correction** : Ajouter un contrôle de ring ou de capability avant d'autoriser le flag :

```rust
if flags & IPC_FLAG_INJECT_SRC_PID != 0 {
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    // Seuls les processus Ring1 (PID ≤ RING1_MAX_PID) peuvent utiliser ce flag
    if !crate::security::is_ring1_pid(caller_pid) {
        return EPERM;
    }
    payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
}
```

---

### P1-3 · BLOC 0 incomplet : `arch/constants.rs` absent, `const_assert!` non déployés

**Fichiers concernés** :
- `kernel/src/arch/` — `constants.rs` inexistant
- `kernel/src/exophoenix/ssr.rs` — import depuis crate externe, pas de `const_assert!` local
- `kernel/src/security/exokairos.rs` — constantes déclarées, aucun `const_assert!`
- `kernel/src/process/signal/handler.rs:151` — `const_assert!` **commenté** : `// const_assert!(core::mem::size_of::<SignalFrame>() <= 4096)`

**Constat** :

Le MASTER-CHECKLIST BLOC 0 définit 13 critères d'outillage d'audit (O-01 à O-13) qui doivent être satisfaits avant tout autre développement. Parmi eux :

- **O-01** : `arch/constants.rs` créé avec toutes les constantes canoniques → **fichier absent**
- **O-02** : `const_assert!` dans `ssr.rs` (SSR size ≤ 4096) → **absent**
- **O-03** : `const_assert!` dans `exokairos.rs` (KAIROS_WINDOW_NS) → **absent**
- **O-04** : `const_assert!` dans `physmap.rs` (PHYSMAP_INITIAL_COVERAGE) → **absent**
- **O-05** : `const_assert!` cohérence `CORE_MASK_WORDS × 64 == MAX_CORES_LAYOUT` → **absent**

Le seul `const_assert!` trouvé dans une position structurelle (`signal/handler.rs:151`) est **commenté**, ce qui indique une intention non réalisée.

**Impact** : Sans ces gardes de compilation, les régressions silencieuses restent invisibles. En particulier : si la SSR dépasse à nouveau 4 KiB lors d'un refactor, aucune alerte ne sera émise au build. Le seuil de passage BLOC 0 → BLOC 1 est `13/13` — l'état actuel est estimé à `0/13`.

**Correction** : Créer `kernel/src/arch/x86_64/constants.rs` avec les constantes canoniques consolidées, et ajouter les gardes `const_assert!` dans chaque module concerné. Exemple minimal pour `ssr.rs` :

```rust
// Garantir que le layout SSR exporté par la crate est dans les bornes
const _: () = assert!(exo_phoenix_ssr::SSR_SIZE <= 16 * 4096,
    "SSR dépasse 16 pages — revoir CORR-81");
```

---

### P1-4 · `init_server` : `network_server` marqué `critical: false` mais timeout 3s bloque le graphe de dépendances

**Fichier concerné** : `servers/init_server/src/service_table.rs:74–79`

**Constat** :

```rust
ServiceMetadata {
    name: "network_server",
    bin_path: NETWORK_SERVER_BIN,
    requires: DEPS_NETWORK,
    requires_optional: NO_DEPS,
    ready_timeout_ms: 3_000,   // ← 3 secondes seulement
    critical: false,
},
```

`network_server` dépend de `virtio_drivers` qui dépend de `device_server`. Si `virtio_drivers` subit un timeout au démarrage (sans `-net` ou sans disque virtio configuré), `network_server` attend 3 secondes avant d'être déclaré mort.

La logique de `supervisor.can_start()` autorise le démarrage d'un service dont une dépendance est morte **si cette dépendance est non-critique**. Cela est correct. Mais `DEPS_NETWORK` déclare `virtio_drivers` comme dépendance **requise** (non-optionnelle) :

```rust
const DEPS_NETWORK: &[&str] = &[
    "ipc_router",
    "vfs_server",
    "device_server",
    "virtio_drivers",   // ← requis, pas optionnel
];
```

Si `virtio_drivers` timeout en 5 secondes, `network_server` ne peut pas démarrer pendant ce temps, bloquant potentiellement le démarrage séquentiel de `exo_shield` (qui attend `network_server` en `OPT_DEPS`). En pratique la résolution correcte se produit mais **avec un délai de 8 secondes minimum** avant que `exosh` soit disponible, même sur une machine sans réseau.

**Impact** : En mode QEMU `-net none`, le boot est artificiellement ralenti de 8 secondes. Pour CORR-79 (exosh démarre sans réseau), le critère B-10 peut techniquement passer mais avec un délai non documenté.

**Correction** : Déplacer `virtio_drivers` dans `requires_optional` pour `network_server`, ou réduire son `ready_timeout_ms` à 1000ms dans le contexte sans réseau.

---

### P1-5 · `#![allow(static_mut_refs)]` : lint global maintenu dans `lib.rs`

**Fichier concerné** : `kernel/src/lib.rs:28`

**Constat** :

```rust
#![allow(static_mut_refs)]
```

Ce lint est toujours présent, identique au rapport précédent (P2-2). Depuis l'itération 1, aucune réduction n'a été effectuée. Or plusieurs modules nouveaux ont été ajoutés dans le snapshot 2026-05-20, potentiellement avec de nouveaux accès non synchronisés masqués par ce lint.

**Risque additionnel** : les 5 nouveaux modules d'ExoShield (hooks, sandbox, network, ml, forensics) qui seront ajoutés suite à P0-3 risquent d'utiliser des statics mutables. Sans ce lint retiré, les avertissements seront masqués dès leur intégration.

**Correction** : Identifier les sites avec `grep -n 'static mut\|&mut.*STATIC'` et corriger progressivement. Objectif : retirer le `#![allow]` global avant le gel de code v0.2.0.

---

## P2 — Incohérences Mineures (portées de l'itération précédente)

### P2-1 · Crates drivers vides toujours membres du workspace

**Répertoires** : `drivers/storage/ahci/src/`, `drivers/storage/nvme/src/`, `drivers/network/e1000/src/`, `drivers/display/virtio_gpu/src/`, `drivers/clock/src/`

Inchangé depuis l'itération 1. Ces fichiers de 0 octet restent dans le workspace principal. Le build et le `Cargo.lock` en sont pollués. Déplacer dans `drivers/future/` reste l'action recommandée.

---

### P2-2 · `userspace/apps/coreutils/` et built-ins `exosh` : double implémentation sans arbitrage

**Fichiers** : `userspace/apps/coreutils/src/bin/` (cat, echo, ls, mkdir, rm, rmdir, touch) vs `servers/exosh/src/main.rs`

Inchangé. Aucune décision de stratégie n'a été documentée. L'ajout de `fork/exec` dans musl-exo (BLOC 6) rendra cette tension plus visible dès que M-03 sera complété.

---

### P2-3 · `COW_TABLE_SIZE = 65536` : saturation silencieuse toujours sans compteur

**Fichier** : `kernel/src/memory/cow/tracker.rs:18`

Inchangé. Aucun compteur atomique d'overflow n'a été ajouté. La préparation à Wayland (v0.3.0) rend ce risque plus concret à chaque commit.

---

### P2-4 · Architecture `aarch64` : compilation trompeuse sans boot réel

**Fichier** : `kernel/src/arch/aarch64/mod.rs:8`

Inchangé. Le placeholder reste compilé sans avertissement. Aucune correction du périmètre officiel n'a été documentée.

---

### P2-5 · `virtio_drivers` server Ring1 : boucle IPC heartbeat uniquement

**Fichier** : `servers/virtio_drivers/src/main.rs`

Inchangé. Le serveur ne traite toujours que `VIRTIO_MSG_HEARTBEAT` et `VIRTIO_MSG_STATUS`. La confusion architecturale avec `drivers/storage/virtio_blk/` persiste. Ce point prend une dimension supplémentaire avec P0-1 : la correction du BAR VirtIO devra clarifier si l'init disk se fait côté kernel (`virtio_adapter.rs`) ou sera migrée vers ce serveur Ring1 dans une future étape.

---

## Bilan par rapport aux MASTER-CORRECTIONS

| CORR | Description | Appliqué ? |
|------|-------------|------------|
| CORR-75-A | `exo_shield/lib.rs` : 5 modules manquants | ❌ **NON** — P0-3 ce rapport |
| CORR-75-B | `exo_shield/main.rs` : init 5 modules | ❌ **NON** |
| CORR-75-C | Hooks dans `handle_event_report()` | ❌ **NON** |
| CORR-75-D | Containment réel dans `handle_quarantine_cmd()` | ❌ **NON** |
| CORR-75-E | YARA patterns 8→64 bytes | ❌ **NON** |
| CORR-75-F | Bridge ExoArgos→exo_shield | ❌ **NON** |
| CORR-76 | physmap > 1 GiB | ⚠️ **Non vérifié** (crate exo-physmap hors zip) |
| CORR-77 | `cgroup::init()` avant runqueue | ✅ **Résolu** (commentaires lib.rs cohérents) |
| CORR-78 | Injection PID len==128 | ✅ **Résolu** (remplacement par `IPC_FLAG_INJECT_SRC_PID`) — surface résiduelle en P1-2 |
| CORR-79 | exosh bloqué par network_server | ⚠️ **Partiellement** — supervisor correct, délai 8s résiduel (P1-4) |
| CORR-80 | `USER_ELF_BASE_MIN` = 64 KiB | ✅ **Résolu** (`USER_START = 0x10000`) |
| CORR-81 | SSR overflow 4 KiB | ⚠️ **Non vérifié** (struct dans crate lib.zip externe) |
| CORR-82 | ExoSeal avant mémoire | ✅ **Résolu** (séquence de boot lib.rs correcte) |
| CORR-83 | wgpu no_std impossible | ✅ **Résolu** (retiré, fontdue documenté) |
| CORR-84 | `is_immutable()` non vérifié | ❌ **NON** — P0-2 ce rapport |
| CORR-85 | Données réseau IPC > 240B | ⚠️ **Non vérifié** (exo-net dans lib.zip) |
| CORR-86 | ExoFS RAM-only / BAR VirtIO | ❌ **NON** — P0-1 ce rapport |

---

## Synthèse — Priorités pour v0.2.0

```
Priorité absolue (BLOC -1 / bloquer la release) :
  P0-1  Lire BAR VirtIO depuis PCI config space (DEFAULT_VIRTIO_BLK_MMIO_BASE)
  P0-2  Vérifier is_immutable() dans write_blob() avant toute écriture ExoFS
  P0-3  Ajouter 5 modules manquants à exo_shield/lib.rs + init dans _start()

À résoudre dans le cycle v0.2.0 :
  P1-1  Mettre à jour doc plage [400..499] dans syscall/numbers.rs
  P1-2  Restreindre IPC_FLAG_INJECT_SRC_PID aux processus Ring1
  P1-3  Créer arch/constants.rs + déployer const_assert! BLOC 0 (O-01 à O-05)
  P1-4  Reclasser virtio_drivers en dépendance optionnelle pour network_server
  P1-5  Réduire progressivement #![allow(static_mut_refs)] dans lib.rs

À documenter et planifier :
  P2-1  Déplacer crates drivers vides dans drivers/future/
  P2-2  Trancher stratégie coreutils vs built-ins exosh
  P2-3  Ajouter COW_OVERFLOW_COUNT atomique dans cow/tracker.rs
  P2-4  Retirer aarch64 du périmètre officiel v0.2.0
  P2-5  Clarifier ownership VirtIO : kernel vs serveur Ring1
```

---

## Remarque sur l'état global BLOC 0

Le seuil de passage BLOC 0 → BLOC 1 est `13/13`. L'état estimé au snapshot 2026-05-20 est **0/13** : aucune des 13 conditions d'outillage d'audit (constants.rs, const_assert!, audit_constants.py, semgrep-rules, deny.toml, pre-commit, CI) n'est satisfaite. Sans ce socle outillage, les régressions de constantes et les violations de dépendances resteront invisibles au CI.

**Recommandation** : traiter BLOC 0 en parallèle des corrections P0, pas après — son retard fait boucler les audits manuels.

---

*— Claude Delta, audit du snapshot kernel.zip 2026-05-20.*  
*Itération 2 — fait suite à `EXOOS_v0.2.0_AUDIT_INCOHERENCES_claude_delta.md` (2026-05-14).*
