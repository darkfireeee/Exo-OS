# Analyse de la Couche Serveurs — ExoOS v0.2.0

**Auteur :** Claude Delta  
**Dépôt :** `https://github.com/darkfireeee/Exo-OS.git`  
**Date :** 2026-06-05  
**Périmètre :** Répertoire `servers/` — 17 serveurs Ring 1 + shell userspace

---

## 1. Vue d'ensemble

ExoOS suit une architecture **microkernel pur** : le noyau ne fournit que l'ordonnancement, la gestion mémoire bas-niveau et les primitives IPC. Toute la politique système est déléguée à des **serveurs Ring 1** qui communiquent exclusivement via `SYS_IPC_SEND` / `SYS_IPC_RECV`. Le userspace (Ring 3) accède aux services par ce même canal.

### 1.1 Carte des 17 serveurs

| PID / Endpoint | Nom                | Rôle principal                                    | LOC  |
|:--------------:|:-------------------|:--------------------------------------------------|:----:|
| 1              | `init_server`      | Superviseur PID 1, démarreur, watchdog            | 1819 |
| 2              | `ipc_router`       | Annuaire IPC, routage, security gate ExoCordon    | 1903 |
| 3              | `vfs_server`       | VFS namespace, mount table, ExoFS/procfs/sysfs    | 2368 |
| 4              | `crypto_server`    | Primitives crypto (XChaCha20, Ed25519, BLAKE3)    | 3491 |
| —              | `memory_server`    | Quotas mémoire, mmap, SHM                        |  789 |
| —              | `device_server`    | Registre PCI, hotplug, IOMMU, power states        | 1051 |
| —              | `input_server`     | Hub clavier/souris, queue circulaire, pub/sub      |  241 |
| —              | `fb_server`        | Console framebuffer, VT100, police bitmap          |  816 |
| —              | `tty_server`       | Discipline de ligne, liaison fb↔input             |  515 |
| —              | `exosh`            | Shell interactif Ring 3                           | 3899 |
| —              | `scheduler_server` | Politique CFS/RT/Deadline, affinité CPU            | 1212 |
| —              | `network_server`   | TCP/UDP/ICMP, smoltcp, DHCP, pool DMA VirtIO      | 3947 |
| —              | `exo_shield`       | Sécurité : scanning, ML, forensics, firewall, IDS | 23842|
| —              | `virtio_drivers`   | Couche abstraction VirtIO                          |  144 |
| —              | `ipc_router`       | (load_balancer, exocordon modules)                 | —    |
| —              | `phase5-tests`     | Tests d'intégration Ring 1                        | 1381 |
| —              | `syscall_abi`      | ABI syscall partagée + contrats de test           | 1226 |

**Total LOC Rust analysés :** ~48 000 lignes

---

## 2. Analyse par serveur

### 2.1 `init_server` — PID 1, Superviseur

**Rôle** : Point d'entrée de l'espace utilisateur. Il est le seul processus qui ne peut pas être tué légitimement (SIGKILL/SIGSTOP non masquables par lui-même, mais non-applicables à PID 1 par le noyau).

**Séquence de démarrage** (ordre canonique) :
```
ipc_router → memory_server → vfs_server → device_server
→ input_server → fb_server → tty_server → ps2_driver
→ exosh → crypto_server → virtio_drivers → e1000_driver
→ virtio_net_driver → loopback_driver → network_server
→ scheduler_server → exo_shield
```

**Points forts :**
- Backoff exponentiel (1 → 2 → 4 → … → 32 ticks) sur crash rapide
- Reset du délai si le service a tenu ≥ 5 secondes (`SERVICE_STABLE_MS = 5000`)
- Plan de contrôle IPC complet : `HEARTBEAT`, `STATUS`, `START`, `STOP`, `RESTART`, `CHILD_DIED`, `PREPARE_ISOLATION`
- Gestion propre SIGTERM (envoi à tous les enfants avant halt)

**Points faibles / risques :**

- **Aucun ordre de dépendance explicite** entre les services dans `init_server` lui-même. L'ordre est codé en dur dans le tableau `SERVICES[]`, sans graphe de dépendance vérifiable à la compilation. Si un service est réordonné par erreur, les dépendances implicites (ex : `vfs_server` avant `device_server`) peuvent être violées silencieusement.
- **`supervisor::can_start`** effectue des vérifications de dépendances, mais cette logique est dans `supervisor.rs`, non visible ici. Si elle est incomplète, un service peut démarrer sans prérequis.
- **Pas de timeout global de boot** : si `boot_sequence::wait_for_ipc_ready` se bloque indéfiniment sur un service gelé (non-crash), le boot se fige.

---

### 2.2 `ipc_router` — PID 2, Annuaire + Security Gate

**Architecture interne :**
```
ipc_router/
├── main.rs          — boucle IPC (REGISTER / ROUTE / HEARTBEAT)
├── router.rs        — table de routage
├── security_gate.rs — application ExoCordon + IPC-04
├── exocordon.rs     — DAG d'autorisation statique (10 ServiceId)
└── load_balancer.rs — équilibrage de charge
```

**Protocole de nommage** : hash FNV-32 sur le nom du service (`b"vfs_server"` → u32). Le registre supporte **64 services** simultanés.

**Politique Zero-Trust (ExoCordon)** :
- DAG statique : `Init → IpcBroker → Memory → Vfs → Crypto → Device → Network → Scheduler → VirtioDrivers → ExoShield`
- Deny-by-default : tout chemin non présent dans le DAG est refusé
- Quota par arête : `quota_left: AtomicU64` (rate-limiting)
- `SOFT_QUARANTINE_THRESHOLD = 10` violations → notification init

**Problèmes identifiés :**

| Sévérité | Problème |
|:--------:|:---------|
| 🔴 BLOQUANT | **Collision FNV-32** : le registre de 64 entrées stocke uniquement le hash 32 bits, pas le nom complet. Deux services avec le même hash (collision possible) s'écraseront mutuellement silencieusement. |
| 🟡 MINEUR | Le **DAG ExoCordon ne couvre que 10 ServiceId** mais init_server démarre 17 services. Les 7 services sans ServiceId (fb, tty, input, exosh, ps2_driver, loopback, e1000) ne passent pas par ExoCordon → zero-trust partiel. |
| 🟡 MINEUR | La limite de **64 entrées** est fixe. Un crash-restart rapide peut saturer le registre si les désinscritions ne sont pas implémentées. |
| ℹ️ INFO | `IPC_MSG_ROUTE` forward **sans validation du payload** au-delà de la security gate (on fait confiance au verdict `Allow`). |

---

### 2.3 `vfs_server` — PID 3, Virtual File System

**Responsabilités** :
- Table de montages (max 32) : ExoFS, ProcFs, SysFs, DevFs
- Résolution chemins → BlobId via `SYS_EXOFS_PATH_RESOLVE` (syscall 500)
- Opérations POSIX délégées au kernel : `read`, `write`, `stat`, `getdents64`, `mkdir`, `rename`, `truncate`, `fsync`

**Points forts :**
- Cross-process I/O propre via `SYS_EXO_MEM_COPY_FROM/TO_PID` (chunks de 4 KiB)
- Contrôle d'accès strict : `VFS_MOUNT` et `VFS_UMOUNT` réservés à PID 1 uniquement

**Problèmes identifiés :**

| Sévérité | Problème |
|:--------:|:---------|
| 🔴 BLOQUANT | **`MountTable` utilise `UnsafeCell` sans mutex** : `unsafe impl Sync for MountTable {}`. Si deux requêtes IPC arrivent simultanément (ex : via SMP ou deux threads kernel), la table est corrompue. Le commentaire indique que c'est un serveur mono-thread, mais rien ne l'impose. |
| 🟡 MINEUR | **Collision FNV-32 dans la mount table** : `path_hash` est un u32. Deux chemins avec le même hash (improbable mais possible) se remplaceraient. |
| ℹ️ INFO | `VFS_RESOLVE` ne retourne que les 64 bits bas du BlobId (`blob_id_low64()`). Pour les BlobId > 64 bits, il faudra un second champ. |

---

### 2.4 `crypto_server` — Endpoint 4, PID 5

**Primitives exposées** (12 opérations + Phoenix) :
```
DERIVE_KEY | RANDOM | ENCRYPT | DECRYPT | HASH
SIGN | VERIFY (streaming 3 phases) | TLS_INIT/HANDSHAKE/CLOSE
KEY_REVOKE | KEY_ROTATE | KEY_REVOKE_OWNER | KEY_STATS
PHOENIX_WAKE_ENTROPY (255)
```

**Stack cryptographique** : `XChaCha20-Poly1305`, `BLAKE3`, `Ed25519` (`ed25519-dalek`), `PKI` interne, `TLS` simplifié

**Points forts :**
- Seul serveur Ring 1 autorisé à manipuler des clés brutes
- Capability tokens requis pour les opérations sensibles (CAP-01)
- Rotation et révocation de clés intégrées
- Support entropy Phoenix (`PHOENIX_WAKE_ENTROPY`) pour la récupération post-crash

**Problèmes identifiés :**

| Sévérité | Problème |
|:--------:|:---------|
| 🟡 MINEUR | `CRYPTO_SERVER_ENDPOINT = 4` mais `CRYPTO_SERVER_PID = 5` — l'endpoint et le PID sont découplés. Si un autre service s'enregistre à l'endpoint 4 avant le crypto_server, le routing sera corrompu. |
| ℹ️ INFO | `VERIFY_CONTEXTS = 4` : au plus 4 vérifications de signature en streaming simultanées. Saturation possible sous charge. |

---

### 2.5 `memory_server` — Politique mémoire

**Services exposés** : `ALLOC`, `FREE`, `PROTECT`, `QUERY`, `SHM_CREATE/ATTACH/DESTROY`, `QUOTA_SET/QUERY`

**Points forts** : quotas par PID, handles opaques (jamais d'adresses physiques exposées), SHM contrôlé

**Problème principal** :

| Sévérité | Problème |
|:--------:|:---------|
| 🟡 MINEUR | Utilisation de `spin::Mutex<MemoryService>` : sur un système SMP chargé, la contention peut provoquer des busy-loops prolongées. Un `Mutex` à suspension serait préférable. |

---

### 2.6 `scheduler_server` — Politique d'ordonnancement

**Classes de scheduling** : `Cfs`, `Realtime`, `Deadline`, `Idle`

**Points forts** :
- Admission control temps réel : utilisation totale bornée en PPM (`total_utilization_ppm`)
- `PolicyAdvisor` : conversion nice/latency_hint → priority_weight
- Séparation claire : le serveur propose, le kernel applique (`SYS_SETPRIORITY`, `SYS_SCHED_SETSCHEDULER`, `SYS_SCHED_SETAFFINITY`)
- Stats par thread (yield_count, error_count, etc.)

**Problème** :

| Sévérité | Problème |
|:--------:|:---------|
| ℹ️ INFO | `apply_kernel_*` fonctions ignorent silencieusement `ENOSYS` — si le kernel ne supporte pas une primitive RT, le scheduler_server continue sans erreur visible. |

---

### 2.7 `network_server` — Réseau V4

**Architecture** :
```
network_server/
├── main.rs          — boucle principale, bootstrap, dispatch
├── smoltcp_iface.rs — interface smoltcp (TCP/UDP/Raw)
├── socket_table.rs  — table de sockets (bornée)
├── tcp_store.rs     — état TCP Phoenix (sérialisation crash-recovery)
├── buf_pool.rs      — pool DMA VirtIO
├── dhcp.rs          — client DHCP
├── routing.rs       — table de routage statique
├── driver_link.rs   — liaison IPC avec le driver réseau
├── icmp.rs          — ICMP
└── isolation.rs     — état Phoenix réseau
```

**Points forts :**
- Intégration smoltcp propre (pas de dépendance C)
- `tcp_store.rs` : sérialisation de l'état TCP pour Phoenix (crash recovery)
- Pool DMA explicite évitant les allocations dynamiques
- IP configurable via `/etc/network.conf` (fallback `10.0.2.15/24`)

**Problèmes identifiés :**

| Sévérité | Problème |
|:--------:|:---------|
| 🔴 BLOQUANT | **`NetworkService` utilise `spin::Mutex`** sur l'intégralité du service. Chaque opération réseau (accept, send, recv) bloque tous les autres appels. Latence catastrophique sous charge IPC concurrente. |
| 🟡 MINEUR | `handle_connect` retourne `EAGAIN` si TCP n'est pas encore établi. L'appelant doit boucler. Pas de notification asynchrone → polling actif côté client. |
| 🟡 MINEUR | `NET_OP_RECVMSG` / `NET_OP_SENDMSG` / `NET_OP_SOCKETPAIR` retournent `EOPNOTSUPP`. Incompatibilité avec certains programmes musl qui les utilisent. |
| ℹ️ INFO | `DEFAULT_IPV4 = 0x0a00020f` (10.0.2.15) est codé en dur comme fallback — spécifique QEMU/VirtIO. |

---

### 2.8 `fb_server` — Console Framebuffer

**Fonctionnalités** : rendu bitmap, VT100 (ESC CSI), SGR couleurs 8, scrolling optimisé (memcpy), progressive clear, curseur clignotant

**Points forts :**
- Import direct de la police depuis `exo-boot` (`shared_font`) — cohérence visuelle garantie
- `scroll_up_pixels` : utilise `core::ptr::copy` pour un scroll O(n) rapide
- Formatage 3bpp/4bpp supporté

**Problèmes identifiés (connus, documentés dans le code) :**

| Sévérité | Problème |
|:--------:|:---------|
| 🟡 MINEUR | **PATCH-FB-01** : `ConsoleCell` utilise `UnsafeCell` sans mutex. L'invariant mono-thread doit être respecté. Si fb_server gagne un second thread (ex: flush asynchrone), c'est un data race immédiat. |
| 🟡 MINEUR | **PATCH-FB-02** : `#[path = "../../../exo-boot/src/display/font.rs"]` est un chemin relatif fragile. Un déplacement de répertoire casse silencieusement la compilation sans erreur claire. |
| ℹ️ INFO | SGR partiel : seules les couleurs 30–37 sont mappées (pas de 90–97 bright, pas de couleurs de fond 40–47). |

---

### 2.9 `tty_server` — Discipline de ligne

**Architecture** :
- Délègue à `exo_tty::LineDiscipline` pour le traitement ANSI/signaux
- Pont `input_server → tty_server → fb_server`
- Support `RawCall` (magic `0x4558_4F43`) pour les reads asynchrones avec `reply_ep + cookie`

**Points forts** :
- `pending_read` : mise en attente d'un read jusqu'à disponibilité d'une ligne complète
- Signaux gérés : `Interrupt (^C)`, `EndOfFile (^D)`, `ClearScreen (^L)`
- Backpressure fb : retry avec `SCHED_YIELD` jusqu'à `FB_SEND_RETRY_LIMIT = 8`

**Problèmes :**

| Sévérité | Problème |
|:--------:|:---------|
| 🟡 MINEUR | **Un seul `pending_read`** : si deux processus tentent un `TTY_MSG_READ_LINE` simultanément, le second reçoit `EAGAIN`. Pas de file d'attente de lectures. |
| ℹ️ INFO | `TtyState` utilise `UnsafeCell` — même invariant mono-thread que fb_server. |

---

### 2.10 `input_server` — Hub d'entrée

**Points forts :**
- Queue circulaire lockfree (mono-thread) de 128 événements
- Modèle pub/sub : `INPUT_MSG_ATTACH` enregistre un seul abonné (`SUBSCRIBER_ENDPOINT`)
- Livraison directe si abonné présent, queue sinon

**Problème majeur :**

| Sévérité | Problème |
|:--------:|:---------|
| 🔴 BLOQUANT | **Un seul abonné** (`AtomicU64 SUBSCRIBER_ENDPOINT`). Si `tty_server` et `exosh` s'attachent tous les deux, le second écrase le premier. Un événement clavier est perdu pour l'un des deux. Multi-applications simultanées impossibles. |

---

### 2.11 `exosh` — Shell interactif

**Commandes intégrées** : `help`, `cd`, `ls`, `mkdir`, `touch`, `cat`, `echo`, `rm`, `cp`, `mv`, `stat`, `tree`, `history`, `time`, `dd`, `sync`, `ping`, `tcping`, `bench (ipc|sched|crypto|fs)`, `shutdown`, `reboot`, `exit`

**Commandes via `execve`** : `sleep`, `top`, `ps`, `kill`, `clear`, `meminfo`, `syscall-stat`, `ipc-stat`, `/bin/*`, `/sbin/*`

**Points forts :**
- Lecture ligne via `SYS_READ` (STDIN → `/dev/pts/0`) avec fallback tty IPC
- Navigatoin historique (16 entrées), curseur gauche/droite via codes ANSI
- Benchmark intégré : IPC round-trip, sched yield, crypto throughput, FS write+read
- `ping` ICMP et `tcping` TCP fonctionnels
- Pipe simple `;` (exécution séquentielle)

**Problèmes :**

| Sévérité | Problème |
|:--------:|:---------|
| 🟡 MINEUR | `read_line` effectue un `SYS_READ` bloquant. Si le TTY n'est pas disponible, il attend 10ms puis retente. Mode interactif correct mais inefficace energétiquement. |
| 🟡 MINEUR | `HISTORY_MAX = 16` lignes. Suffisant pour usage courant. |
| ℹ️ INFO | `EXEC_MAX_ARGS = 32` avec `EXEC_ARG_MAX = 160` bytes/arg → limite max ~5 Ko d'arguments. Suffisant pour la plupart des cas. |

---

### 2.12 `device_server` — Contrôleur PCI/Hotplug

**Services** : `REGISTER_DEVICE`, `CLAIM`, `RELEASE`, `QUERY`, `POWER_SET`, `EVENT_POLL`, `FAULT`

**Points forts :**
- `validate_claim` : vérification des claims drivers avant propagation au kernel
- Queue hotplug bornée, journalisation des événements
- `IommuLedger` : suivi des mappings DMA
- `PowerPolicyTable` : politiques D0–D3

---

### 2.13 `exo_shield` — Serveur de Sécurité

**Envergure** : 23 842 lignes, 37 fichiers Rust — le plus grand serveur du projet.

**Modules** :
```
exo_shield/
├── engine/   — core, scanner, realtime (scoring, signatures, profils)
├── behavioral/ — anomaly, heuristic, profiler, sequence
├── forensics/  — memory_dump, timeline, report
├── hooks/    — exec, memory, net, syscall hooks
├── ipc_gate/ — access, policy, audit
├── ml/       — features, inference, model, update
├── network/  — dns_guard, firewall, ids, traffic_analysis
└── sandbox/  — container, fs_restriction, net_isolation, syscall_filter
```

**Points forts :**
- Scoring de menaces temps réel
- Quarantaine de processus (`QUARANTINE_CMD`)
- Forensics : timeline, memory dump, rapport
- ML inference (modèle embarqué, no_std)
- IDS réseau + DNS guard + firewall
- Sandbox par processus (filtrage syscall, isolation réseau/FS)

**Note** : La taille de ce serveur (24K lignes) suggère qu'il porte une grande partie de la logique de sécurité qui, idéalement, serait partiellement dans le kernel ou vérifiée formellement. Une revue de sécurité approfondie est recommandée avant production.

---

## 3. Problèmes bloquants récapitulatifs

| # | Serveur | Problème | Impact |
|:-:|:--------|:---------|:-------|
| P1 | `ipc_router` | **Collision FNV-32** dans le registre : hash 32 bits sans vérification de collision | Services indistinguables, routing corrompu |
| P2 | `vfs_server` | **`MountTable` non protégée** (`UnsafeCell` sans mutex) | Corruption de la table de montages sous SMP |
| P3 | `network_server` | **`spin::Mutex` global** sur l'intégralité du NetworkService | Contention catastrophique sous charge IPC |
| P4 | `input_server` | **Un seul abonné** possible (`SUBSCRIBER_ENDPOINT`) | Impossible d'avoir plusieurs applications clavier simultanées |

---

## 4. Problèmes modérés récapitulatifs

| # | Serveur | Problème |
|:-:|:--------|:---------|
| M1 | `fb_server` | PATCH-FB-01 : `UnsafeCell<Console>` sans mutex |
| M2 | `fb_server` | PATCH-FB-02 : chemin `#[path]` relatif fragile vers `exo-boot` |
| M3 | `ipc_router` | DAG ExoCordon incomplet (10/17 services) |
| M4 | `tty_server` | Un seul `pending_read` (pas de file) |
| M5 | `network_server` | `connect` TCP non-bloquant → polling actif côté client |
| M6 | `network_server` | `SENDMSG`/`RECVMSG`/`SOCKETPAIR` non implémentés |
| M7 | `crypto_server` | Découplage endpoint (4) / PID (5) — risque de confusion |
| M8 | `memory_server` | `spin::Mutex` — busy-wait sous contention |
| M9 | `init_server` | Pas de timeout global de boot |

---

## 5. Points forts architecturaux

1. **Isolation totale** : chaque serveur tourne en espace d'adressage séparé (Ring 1). Un crash de `network_server` ne corrompt pas `vfs_server`.

2. **Phoenix / crash-recovery** : `network_server` sérialise l'état TCP complet (`tcp_store.rs`, 6 176 octets/socket), permettant une reprise après panic sans perte de connexions.

3. **Zéro allocation dynamique** en dehors de `memory_server` et `exo_shield` — tous les serveurs critiques sont `no_std` et travaillent sur des structures statiques bornées.

4. **ExoCordon** : le DAG statique de politiques IPC est le mécanisme de confinement central — lisible, vérifiable statiquement, mirroir exact de `kernel/src/security/ipc_policy.rs`.

5. **Crypto isolé** : `crypto_server` est le seul point d'entrée pour les primitives cryptographiques. Le key material n'est jamais transmis en clair via IPC (handles opaques).

6. **Init robuste** : le backoff exponentiel avec réinitialisation sur service stable est une politique de supervision professionnelle. Un service instable ne monopolise pas les ressources.

7. **ExoShield complet** : la couche de sécurité couvre scanning, behavioural analysis, ML, forensics, réseau, sandbox — bien au-delà d'un OS expérimental standard.

---

## 6. Recommandations prioritaires (avant v0.2.0)

### R1 — Corriger la collision FNV-32 dans `ipc_router` [CRITIQUE]
Stocker le nom complet (ou un hash 64 bits) dans le registre et valider l'identité lors du lookup. Une table `(hash_u64, name_bytes[16], endpoint)` résout le problème.

### R2 — Protéger `MountTable` dans `vfs_server` [CRITIQUE]
Remplacer `UnsafeCell<[MountEntry]>` par `spin::Mutex<[MountEntry]>` ou documenter formellement l'invariant mono-thread avec une assertion de démarrage.

### R3 — Découpler le mutex du `network_server` [IMPORTANT]
Le mutex global `NETWORK_SERVICE` doit être découpé : un verrou par socket pour les opérations data-plane, un verrou global uniquement pour les opérations control-plane (open/close/listen).

### R4 — Support multi-abonnés dans `input_server` [IMPORTANT]
Remplacer `AtomicU64 SUBSCRIBER_ENDPOINT` par une table d'abonnés (ex: 4 entrées) avec diffusion par copie. Requis pour le futur support Wayland/multi-fenêtres.

### R5 — Compléter le DAG ExoCordon [MODÉRÉ]
Ajouter les `ServiceId` manquants : fb, tty, input, exosh, ps2_driver, loopback, e1000. La sécurité zéro-trust doit couvrir la totalité de la surface IPC.

### R6 — Externaliser la police bitmap de `fb_server` [MINEUR]
Créer un crate `exo-font` indépendant importé par `exo-boot` et `fb_server`. Éliminer le `#[path]` relatif fragile.

---

*Rapport généré par analyse statique du code source Rust du dépôt ExoOS. Aucune exécution dynamique n'a été effectuée.*
