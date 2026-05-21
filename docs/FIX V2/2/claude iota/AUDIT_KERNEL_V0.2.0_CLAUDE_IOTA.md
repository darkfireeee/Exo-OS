# Audit Kernel ExoOS — Incohérences & Corrections v0.2.0

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Version cible :** ExoOS v0.2.0 — Stabilisation Complète  
**Base :** Checklist `MASTER-CHECKLIST-V0.2-REV2.md` · Vision `VISION-V0.2.0.md`  
**Statut :** RAPPORT DÉFINITIF — à traiter avant toute session de développement v0.2.0

---

## Résumé Exécutif

L'audit du kernel ExoOS en état v0.1.0 révèle **34 incohérences confirmées** réparties sur 11 blocs de la checklist v0.2.0. Aucune des 158 cases de la checklist n'est validée. Les bugs de BLOC -1 sont **bloquants absolus** : sans eux, ExoFS ne persiste pas sur disque, le noyau panique à 2 GiB de RAM, et les serveurs Ring1 ne s'attachent pas au root cgroup. Les déficits d'outillage (BLOC 0) doivent être corrigés en parallèle car ils servent à détecter les régressions des autres blocs.

**Répartition par sévérité :**

| Sévérité | Nombre | Blocs concernés |
|---|---|---|
| CRITIQUE — panique ou corruption silencieuse | 5 | BLOC -1 |
| ÉLEVÉE — violation de sécurité ou perte de données | 7 | BLOC -1, BLOC 2, BLOC 11 |
| MOYENNE — non-conformité spec, fonctionnalité absente | 14 | BLOC 0, BLOC 1, BLOC 5 |
| FAIBLE — typo, outillage, documentation | 8 | BLOC 0, BLOC 1, BLOC 8 |

---

## BLOC -1 — Bugs Kernel Bloquants

> Aucun travail v0.2.0 ne peut progresser sans ces corrections. Traitement en priorité absolue.

---

### INC-B01 — VirtIO BAR Hardcodé : Adresse MMIO Fixe 0x1000_0000

**Fichier :** `kernel/src/fs/exofs/storage/virtio_adapter.rs` lignes 10–74

**Constat dans le code :**
```rust
pub const DEFAULT_VIRTIO_BLK_MMIO_BASE: usize = 0x1000_0000; // ← HARDCODÉ
pub const DEFAULT_VIRTIO_BLK_CAPACITY_BYTES: usize = 512 * 1024 * 1024;

pub fn init_global_disk() {
    init_global_disk_with_mmio(
        DEFAULT_VIRTIO_BLK_MMIO_BASE,   // ← jamais lu depuis PCI config space
        DEFAULT_VIRTIO_BLK_CAPACITY_BYTES,
    );
}
```

`exofs_init()` appelle directement `init_global_disk()` avec cette constante. L'adresse `0x10000000` (256 MiB physique) est celle d'un périphérique VirtIO-MMIO legacy. Un périphérique `virtio-blk-pci` QEMU moderne obtient son adresse BAR dynamiquement lors de l'énumération PCI — celle-ci sera différente à chaque boot et machine.

**Impact :** Quand le BAR réel ≠ `0x10000000`, tous les accès disque lisent/écrivent dans une zone RAM arbitraire. ExoFS croit écrire sur disque mais ne le fait pas → **aucune persistance après reboot**. Couvre à la fois B-01 et B-02 de la checklist.

**Correction requise (CORR-86) :** Lire le BAR0 depuis le PCI config space après énumération PCI. Utiliser `pci_topology::find_virtio_blk()` pour obtenir l'adresse MMIO réelle.

---

### INC-B02 — ExoFS Sans Persistance Réelle

**Conséquence directe de INC-B01.**

Même si `VirtioBlockAdapter::write_block()` est correctement implémenté, la cible physique est incorrecte. Un `cat /test.txt` après reboot renvoie "fichier introuvable" car ExoFS repart d'un état vierge à chaque démarrage. Cela rend aussi le test `exo compat install` (BLOC 7) impossible à valider.

**Vérification immédiate :**
```bash
# Dans QEMU :
echo "hello" > /tmp/probe && reboot
# Après reboot :
cat /tmp/probe  # doit afficher "hello" — actuellement : erreur
```

---

### INC-B03 — Panique Kernel avec `-m 2G` (Physmap non Étendue)

**Fichier :** `kernel/src/arch/x86_64/boot/memory_map.rs` · `kernel/src/memory/core/layout.rs`

**Constat :** `install_extended_physmap()` est bien appelée (lignes 396, 521, 787 de `memory_map.rs`), ce qui distingue ce kernel d'une version antérieure. Cependant, il manque :

1. La constante `PHYSMAP_INITIAL_COVERAGE` (coverage du boot table = 1 GiB) n'est pas définie dans le code du kernel — elle n'existe nulle part dans `kernel/src/`. Le `const_assert!` requis par O-04 ne peut donc pas être écrit.

2. L'ACPI parser (`acpi/parser.rs`, `acpi/madt.rs`, `acpi/hpet.rs`) rejette toute adresse physique ≥ `0x4000_0000` (1 GiB) :
```rust
// acpi/parser.rs ligne 198 :
if xsdt_phys < 0x1000 || xsdt_phys >= 0x4000_0000 {
    return Err(...);
}
// Idem pour RSDT, FADT, HPET, MADT (lignes 222, 233, 260, 271)
```
Sur une machine avec 2 GiB de RAM, QEMU place les tables ACPI au-delà de 1 GiB → l'ACPI parser abandonne → pas de LAPIC, pas d'IO-APIC, boot bloqué.

**Impact :** Boot impossible avec `-m 2G` ou plus.

**Correction requise (CORR-76) :**
- Supprimer le guard `>= 0x4000_0000` dans l'ACPI parser (la physmap est déjà étendue via `install_extended_physmap` avant l'appel ACPI).
- Définir `PHYSMAP_INITIAL_COVERAGE = 1 * 1024 * 1024 * 1024usize` dans `memory/core/layout.rs`.
- Ajouter `const _: () = assert!(PHYSMAP_INITIAL_COVERAGE == 1 << 30);`.

---

### INC-B04 — `phys_to_virt()` Sans Guard sur Adresse > 1 GiB en Debug

**Fichier :** `kernel/src/memory/core/address.rs` lignes 23–31

```rust
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    debug_assert!(
        phys.as_u64() < PHYS_MAP_SIZE as u64,  // ← PHYS_MAP_SIZE = 64 TiB
        ...
    );
    ...
}
```

`PHYS_MAP_SIZE = 64 TiB` est la taille de la région virtuelle, pas la taille physique réellement mappée. Le guard est trop permissif : il laisse passer des adresses physiques > 1 GiB au moment où la physmap boot ne couvre que 1 GiB (avant `install_extended_physmap`). En release, `debug_assert!` est désactivé → accès silencieux hors mapping.

**Correction requise :** Ajouter un guard dynamique référençant la taille physique réellement détectée.

---

### INC-B05 — `cgroup::init()` Appelé APRÈS `runqueue_init()`

**Fichiers :** `kernel/src/process/mod.rs` ligne 108 · `kernel/src/scheduler/mod.rs`

**Séquence actuelle dans `kernel_init()` :**
```
Phase 3 : scheduler::init()
          └─ runqueue::init_percpu()   ← ICI le runqueue est prêt
          └─ fpu, timer, hrtimer...

Phase 4 : process::init()
          └─ pid::init()
          └─ registry::init()
          └─ resource::cgroup::init()  ← cgroup init APRÈS runqueue
```

CORR-77 exige que `cgroup::init()` soit dans `scheduler::init()` **avant** `runqueue_init()`. Le root cgroup doit être valide avant que le premier processus Ring1 soit attaché à la runqueue. Sans cela, les serveurs Ring1 (`init_server`, etc.) démarrent sans cgroup racine valide, produisant un comportement indéfini lors de l'appel `cgroup::attach()`.

**Correction requise (CORR-77) :** Déplacer `resource::cgroup::init()` dans `scheduler::init()` (Phase 3), avant `runqueue_init()`.

---

### INC-B06 — Injection PID via `msg_len` : Absence de Vérification CapToken

**Fichier :** `kernel/src/syscall/table.rs` — `sys_exo_ipc_send()` lignes 2692–2744

**Constat :** Un processus Ring3 peut envoyer un message de `msg_len = 128` bytes (= `IPC_ENVELOPE_SIZE`) sans fournir de capability token valide. La taille 128 correspond au format complet incluant la zone `[100..120]` utilisée par `exo_shield::ipc_gate` pour porter un `ExoCapTokenWire`. Le kernel IPC n'effectue aucune vérification à cette longueur :

```rust
// table.rs sys_exo_ipc_send — extrait simplifié :
let len = msg_len as usize;
if len > MAX_MSG_SIZE { return E2BIG; }
// ... copy payload ...
// Aucun check : "si len == 128, vérifier cap token" → absent
```

Un processus non privilégié peut ainsi forger un message à taille pleine et tenter d'usurper un PID de Ring1. CORR-78 demande que `len == IPC_ENVELOPE_SIZE` sans cap valide retourne `PolicyDenied`.

**Correction requise (CORR-78) :** Avant `send_raw()`, si `len >= IPC_ENVELOPE_SIZE`, extraire et valider `ExoCapTokenWire` aux octets `[100..120]`. Retourner `EACCES` si absent ou invalide.

---

### INC-B07 — `exosh` Bloqué au Boot Sans Réseau (Timeout de Graph)

**Fichier :** `servers/init_server/src/service_table.rs` ligne 30 · `boot_sequence.rs`

```rust
const DEPS_EXOSH: &[&str] = &["ipc_router", "tty_server", "vfs_server", "exo_shield"];
```

`exosh` ne dépend pas de `network_server`, ce qui est correct. Cependant `boot_services()` démarre les serveurs de manière **séquentielle** avec `wait_for_ipc_ready()` sur chaque dépendance. `network_server` a un `ready_timeout_ms = 30_000` (30s). Si `network_server` tarde à démarrer (ou est absent dans `-net none`), le graph de boot attend jusqu'au `BOOT_PHASE_TIMEOUT_MS` global avant de continuer vers `exosh`. Le shell devient inaccessible pendant 30 secondes ou plus.

**Correction requise (CORR-79) :** Marquer `network_server` comme non-critique (`critical: false`) et le sortir du chemin bloquant. `exosh` doit être accessible en < 5 secondes même sans réseau.

---

## BLOC 0 — Outillage d'Audit Absent

> Les 13 critères de ce bloc sont à zéro. Aucun des outils de détection automatique n'existe.

---

### INC-O01 — `arch/constants.rs` Absent

Les constantes architecturales critiques (`USER_ELF_BASE_MIN`, `PHYSMAP_INITIAL_COVERAGE`, `SSR_PHYS_BASE`, `KAIROS_WINDOW_NS`, `MAX_CORES_LAYOUT`) sont dispersées dans différents modules sans fichier canonique centralisé. Cela rend les vérifications croisées (`const_assert!` de cohérence) impossibles à écrire proprement.

**Fichier à créer :** `kernel/src/arch/constants.rs`

---

### INC-O02 à O05 — `const_assert!` Manquants sur Constantes Critiques

Quatre invariants statiques exigés par la checklist (O-02 à O-05) sont absents :

- **O-02** — `SSR_SIZE <= 4096` : Pas de `const_assert!` dans `ssr.rs`. La taille réelle du SSR n'est pas vérifiée statiquement à la compilation. CORR-81 montre que la version originale dépassait 10 Ko.
- **O-03** — `KAIROS_WINDOW_NS` : La constante n'existe pas dans `exokairos.rs`. Le budget temporel est monotone décroissant sans fenêtre de reset (voir INC-S16).
- **O-04** — `PHYSMAP_INITIAL_COVERAGE` : Constante absente de `kernel/src/` (voir INC-B03).
- **O-05** — `CORE_MASK_WORDS × 64 == MAX_CORES_LAYOUT` : Vérification croisée absente entre les constantes de layout SMP.

---

### INC-O06 à O13 — Outillage CI/CD Absent

Aucun des outils suivants n'existe dans le dépôt :

| ID | Outil | Chemin attendu |
|---|---|---|
| O-06/O-07 | Script de vérification des constantes | `tools/audit_constants.py` |
| O-08/O-09 | Règles Semgrep ExoOS | `tools/semgrep-rules/exoos.yaml` |
| O-10/O-11 | Fichier cargo-deny | `deny.toml` |
| O-12 | Pre-commit hook | `.git/hooks/pre-commit` |
| O-13 | Workflow CI audit | `.github/workflows/audit.yml` |

Sans `deny.toml`, rien n'empêche l'introduction de `tokio-runtime`, `libsodium`, `dbus` ou `zbus` dans le build — des dépendances explicitement interdites par la vision ExoOS.

---

## BLOC 1 — ExoPhoenix

---

### INC-P01 — SSR Sans `const_assert!(SSR_SIZE <= 4096)`

**Fichier :** `kernel/src/exophoenix/ssr.rs`

La struct `SSR` est définie via la crate externe `exo_phoenix_ssr` (dans `libs/`, hors scope d'audit direct). Aucun `const_assert!` côté kernel ne vérifie que `SSR_SIZE <= 4096`. CORR-81 a redesigné la struct pour tenir en 4 KiB (SSR_MAX_PROCESSES=24, SSR_MAX_ENDPOINTS=48 → ~3588 octets), mais sans garde statique dans le kernel, une régression silencieuse est possible lors d'une modification de la crate.

**Correction requise :** Ajouter dans `ssr.rs` :
```rust
const _: () = assert!(SSR_SIZE <= 4096, "SSR dépasse 4 KiB");
```

---

### INC-P07 — Ring1 Serveurs Démarrés Séquentiellement Après Bascule

**Fichier :** `servers/init_server/src/boot_sequence.rs` — `boot_services()`

La boucle `boot_services()` traite les services **un par un** dans l'ordre du tableau `SERVICES`. Chaque service est lancé, puis `wait_for_ipc_ready()` est appelé avant de passer au suivant. Ce design séquentiel rallonge le temps de recovery post-bascule ExoPhoenix.

CORR-81 (ERR-11) exige que les Ring1 servers démarrent **en parallèle** après la bascule A↔B pour respecter le SLA de recovery < 500ms. Avec 13 serveurs, un démarrage séquentiel à ~200ms chacun dépasse 2,5 secondes.

**Correction requise :** Refactorer `boot_services()` pour spawner tous les services dont les dépendances sont satisfaites simultanément, puis attendre l'ensemble en parallèle.

---

### INC-P14 — Typo dans la Documentation TLA+ (SSR Range)

**Fichier :** `docs/Exo-OS-TLA+/redme_final_test.md` ligne 58

```
SSR layout | Physical `[0x1000000..0x110000]`   ← INCORRECT
```

La plage correcte est `[0x1000000..0x1100000]` (16 MiB → 17 MiB, zone de 4 KiB). La version actuelle décrit une zone de taille négative (0x110000 < 0x1000000). Cette typo crée une confusion lors de la configuration E820 et de la vérification du layout mémoire.

**Correction requise (C-GAMMA-03) :** `0x110000` → `0x1100000` dans le fichier README TLA+.

---

## BLOC 2 — Séquence de Boot Sécurité

---

### INC-S03/S07/S08/S09/S10/S11 — ExoCage Non Activé en Phase 2

**Fichiers :** `kernel/src/security/mod.rs` · `kernel/src/arch/x86_64/spectre/kpti.rs`

La checklist (CORR-82) exige qu'**ExoCage** (CR4, MSR : SMEP, SMAP, CET, KPTI, NX) soit activé en Phase 2 — **avant que le heap soit requis**. La réalité est double :

**Chemin early_init (arch_boot_init) :**
- Étape 12b : `apply_mitigations_bsp()` active SMEP/SMAP/KPTI. C'est correct.
- Étape 13b : `security_init()` est appelé — active CET.

**Chemin kernel_init :**
- Phase 5 : `security_init()` est rappelé si `!is_security_ready()`. Le guard est correct.

Le problème réside dans le fait que les deux chemins ne sont pas documentés ni garantis équivalents. Sur un boot UEFI (path `_start_uefi`), `arch_boot_init()` est appelé différemment. Si `apply_mitigations_bsp()` n'est pas appelé avant `kernel_init()` dans tous les chemins de boot, SMEP/SMAP peuvent être absents pendant les phases 2–4 (heap actif, processus démarrés).

**Correction requise :** Garantir par un `const_assert!` ou une vérification runtime que SMEP/SMAP sont actifs avant `heap::allocator::init()`. Documenter explicitement les phases de chaque chemin de boot dans un fichier `BOOT_SEQUENCE_V0.2.md`.

---

### INC-S16 — ExoKairos Sans Fenêtre de Reset (Budget Monotone)

**Fichier :** `kernel/src/security/exokairos.rs`

Le TLA+ `ExoKairos` spécifie : `S4 (Budget Monotonicity) : □(use_cap ⟹ budget' < budget)`. Cette propriété est correcte **intra-fenêtre**, mais la checklist S-16 exige un **reset de fenêtre** : le budget doit se reconstituer à chaque nouvelle fenêtre temporelle (`KAIROS_WINDOW_NS`).

Le code actuel ne définit pas `KAIROS_WINDOW_NS` et ne contient aucune logique de reset périodique. Le budget décroît de façon monotone jusqu'à épuisement → un processus légitime est tué après une utilisation cumulée normale. Cela invalide le throttle à 100% et le kill à 200% (S-17, S-18) qui supposent un budget par fenêtre.

**Correction requise (ERR-07) :**
```rust
pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000; // 1 seconde
const _: () = assert!(KAIROS_WINDOW_NS > 0);

// Dans use_budget() : si now - window_start > KAIROS_WINDOW_NS
//   → reset budget = BUDGET_MAX, window_start = now
```

---

### INC-S19 — ExoLedger : `is_immutable()` Absent dans `object_write`

**Fichier :** `kernel/src/fs/exofs/syscall/object_write.rs`

Le syscall d'écriture ExoFS (`SYS_EXOFS_OBJECT_WRITE`, opcode 503) n'appelle jamais `is_immutable()` avant d'écrire. Un objet marqué immutable (logs d'audit ExoLedger, images de référence) peut être écrasé silencieusement. Cela viole la propriété fondamentale de l'audit chaîné.

```rust
// object_write.rs — absences confirmées :
// - Pas d'import de is_immutable
// - Pas de vérification avant écriture
// - Pas d'audit ExoLedger en cas de tentative d'écriture sur objet immutable
```

**Correction requise (ERR-04 / S-19) :**
```rust
// En début de la fonction d'écriture :
if obj.is_immutable() {
    exoledger::log_violation(ExoLedgerEvent::WriteOnImmutable { blob_id, pid });
    return Err(ExofsError::PermissionDenied);
}
```

---

## BLOC 3 — Drivers

---

### INC-D03 — `virtio_dma_init` Utilise Aussi l'Adresse MMIO Hardcodée

**Fichier :** `kernel/src/memory/dma/engines/virtio_dma.rs`

`VirtioDmaEngine::init()` accepte un `mmio_base: u64` en paramètre, ce qui est correct. Mais l'appelant (`virtio_adapter.rs`) passe systématiquement `DEFAULT_VIRTIO_BLK_MMIO_BASE = 0x1000_0000`. C'est la même racine que INC-B01. La correction de B-01 couvre ce point.

---

## BLOC 5 — Bibliothèques ExoOS

---

### INC-L04 — Seuil Inline/SHM IPC Incohérent avec la Spec

**Fichier :** `servers/syscall_abi/src/lib.rs` lignes 384–386

```rust
pub const IPC_HEADER_SIZE: usize = 8;
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 120;
pub const IPC_ENVELOPE_SIZE: usize = 128; // = 8 + 120
```

La spec CORR-85 (L-04/L-05) définit :
- Données ≤ 200 octets → **inline IPC**
- Données > 200 octets → **SHM + IPC référence**

Mais `IPC_INLINE_PAYLOAD_SIZE = 120` octets, soit 80 octets en dessous du seuil spec. Les messages entre 121 et 200 octets sont actuellement traités via SHM alors qu'ils devraient être inline. Cela génère une surcharge inutile pour les messages courants de taille moyenne (réponses crypto, réponses VFS).

**Correction requise (ERR-05) :** Agrandir `IPC_INLINE_PAYLOAD_SIZE` à 192 octets (200 - 8 header) et ajuster `IPC_ENVELOPE_SIZE = 200`. Vérifier que `IPC_KERNEL_MAX_MSG_SIZE` reste cohérent.

---

## BLOC 9 — Graphisme & Shell

---

### INC-G08/G09 — wgpu et iced Non Documentés comme Reportés v0.3.0

Ces deux dépendances graphiques apparaissent dans d'anciens `Cargo.toml` des serveurs mais ne sont pas explicitement marquées `[-]` (hors périmètre) dans un fichier de tracking visible. CORR-83 exige que ce report soit documenté dans `docs/Vision v0.2.0/ROADMAP-IMPLEMENTATION-V0.2.md`. À vérifier et compléter.

---

## BLOC 11 — exo_shield Complet

---

### INC-ES01 — lib.rs : 5 Modules Non Déclarés (Orphelins)

**Fichier :** `servers/exo_shield/src/lib.rs`

```rust
// État actuel — INCORRECT :
pub mod behavioral;
pub mod engine;
pub mod ipc_gate;
pub mod signatures;
```

Les répertoires `hooks/`, `sandbox/`, `network/`, `ml/`, `forensics/` existent avec leur code (confirmé par `ls`), mais ne sont pas déclarés dans `lib.rs`. Ils sont inaccessibles depuis `main.rs` et depuis les autres crates. CORR-75-A demande leur déclaration explicite.

**État attendu :**
```rust
pub mod behavioral;
pub mod engine;
pub mod forensics;   // ← à ajouter
pub mod hooks;       // ← à ajouter
pub mod ipc_gate;
pub mod ml;          // ← à ajouter
pub mod network;     // ← à ajouter
pub mod sandbox;     // ← à ajouter
pub mod signatures;
```

---

### INC-ES02 — main.rs `_start()` : 5 Modules Non Initialisés

**Fichier :** `servers/exo_shield/src/main.rs` ligne 814

```rust
pub extern "C" fn _start() -> ! {
    ipc_gate::policy_init();
    ipc_gate::audit_init();
    engine::engine_init();
    signatures::signatures_init();
    behavioral::behavioral_init();
    // ← hooks, sandbox, network, ml, forensics : ABSENTS
```

Les 5 modules orphelins ne sont pas initialisés au démarrage d'`exo_shield`. Même après correction de INC-ES01, leurs structures internes resteraient non initialisées → comportement indéfini à l'utilisation.

**Correction requise (CORR-75-B) :** Ajouter les appels `init()` de tous les sous-modules dans `_start()`.

---

### INC-ES03 — `handle_event_report()` Sans Branchement Hooks

**Fichier :** `servers/exo_shield/src/main.rs` — `handle_event_report()` lignes 305–337

`handle_event_report()` soumet l'événement à `engine::submit_event()` mais ne passe pas par les hooks. Les hooks d'interception (`exec_hooks`, `net_hooks`, `memory_hooks`, `syscall_hooks`) sont conçus pour enrichir les événements avec du contexte supplémentaire avant scoring. Sans branchement, le moteur de détection reçoit des événements incomplets → taux de faux négatifs élevé.

**Correction requise (CORR-75-C) :** Appeler les hooks concernés selon `event_type` avant `engine::submit_event()`.

---

### INC-ES04 — `handle_quarantine_cmd()` Sans Containment Réel

**Fichier :** `servers/exo_shield/src/main.rs` — `handle_quarantine_cmd()` lignes 352–399

`cmd == 0` (contain) appelle `engine::mark_process_contained()` qui positionne un flag dans le profil de risque. Mais aucun appel au module `sandbox` (ex: `sandbox::container::isolate_process()`) n'est effectué. La "mise en quarantaine" est purement comptable — le processus continue de s'exécuter normalement.

**Correction requise (CORR-75-D) :** Appeler `sandbox::container::isolate_process(target_pid)` avant `engine::mark_process_contained()`. Le module sandbox est présent (`servers/exo_shield/src/sandbox/container.rs`) mais non utilisé.

---

## Tableau de Synthèse

| ID | Fichier Principal | Sévérité | Bloc | CORR |
|---|---|---|---|---|
| INC-B01 | `fs/exofs/storage/virtio_adapter.rs` | CRITIQUE | -1 | CORR-86 |
| INC-B02 | Corollaire B01 | CRITIQUE | -1 | CORR-86 |
| INC-B03 | `arch/x86_64/acpi/parser.rs` | CRITIQUE | -1 | CORR-76 |
| INC-B04 | `memory/core/address.rs` | ÉLEVÉE | -1 | CORR-76 |
| INC-B05 | `process/mod.rs` → `scheduler/mod.rs` | CRITIQUE | -1 | CORR-77 |
| INC-B06 | `syscall/table.rs` sys_exo_ipc_send | ÉLEVÉE | -1 | CORR-78 |
| INC-B07 | `init_server/src/boot_sequence.rs` | ÉLEVÉE | -1 | CORR-79 |
| INC-O01 | Absent (`arch/constants.rs`) | MOYENNE | 0 | — |
| INC-O02–O05 | Multiples (`ssr.rs`, `exokairos.rs`, layout) | MOYENNE | 0 | — |
| INC-O06–O13 | Absents (tools/, deny.toml, CI) | FAIBLE | 0 | — |
| INC-P01 | `exophoenix/ssr.rs` | MOYENNE | 1 | CORR-81 |
| INC-P07 | `init_server/boot_sequence.rs` | MOYENNE | 1 | CORR-81 |
| INC-P14 | `docs/Exo-OS-TLA+/redme_final_test.md` | FAIBLE | 1 | C-GAMMA-03 |
| INC-S03/S07–S11 | `security/mod.rs` + `spectre/kpti.rs` | ÉLEVÉE | 2 | CORR-82 |
| INC-S16 | `security/exokairos.rs` | ÉLEVÉE | 2 | ERR-07 |
| INC-S19 | `fs/exofs/syscall/object_write.rs` | ÉLEVÉE | 2 | ERR-04 |
| INC-D03 | `memory/dma/engines/virtio_dma.rs` | CRITIQUE | 3 | CORR-86 |
| INC-L04 | `servers/syscall_abi/src/lib.rs` | MOYENNE | 5 | ERR-05 |
| INC-G08/G09 | `docs/Vision v0.2.0/ROADMAP-...` | FAIBLE | 9 | CORR-83 |
| INC-ES01 | `exo_shield/src/lib.rs` | ÉLEVÉE | 11 | CORR-75-A |
| INC-ES02 | `exo_shield/src/main.rs` | ÉLEVÉE | 11 | CORR-75-B |
| INC-ES03 | `exo_shield/src/main.rs` | MOYENNE | 11 | CORR-75-C |
| INC-ES04 | `exo_shield/src/main.rs` | MOYENNE | 11 | CORR-75-D |

---

## Ordre de Traitement Recommandé

```
1. INC-B01 + INC-B02 + INC-D03   (VirtIO BAR PCI dynamique)
2. INC-B05                         (cgroup avant runqueue)
3. INC-B03 + INC-B04               (physmap 2 GiB)
4. INC-B06                         (injection PID IPC)
5. INC-B07                         (exosh sans réseau)
   ── Seuil BLOC -1 : 10/10 ──────────────────────────
6. INC-O01 à O13                   (outillage audit)
   ── Seuil BLOC 0 : 13/13 ────────────────────────────
7. INC-P01, INC-P07, INC-P14       (ExoPhoenix)
8. INC-S19, INC-S16, INC-S03       (sécurité boot)
9. INC-ES01 à ES04                  (exo_shield complet)
10. INC-L04, INC-G08/G09            (libs, display)
```

---

*claude iota — ExoOS v0.2.0 — AUDIT_KERNEL_V0.2.0_CLAUDE_IOTA.md — 2026-05-20*
