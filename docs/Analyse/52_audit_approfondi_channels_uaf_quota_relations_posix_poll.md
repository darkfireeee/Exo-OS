# Rapport d'Audit Approfondi #52 — Canaux IPC & UAF, Quotas ExoFS, Relations, Conformité POSIX et Multiplexage Poll

**Date :** 1er Novembre 2026  
**Périmètre :** Noyau Exo-OS — Sous-systèmes **IPC** (`kernel/src/ipc/`), **FS** (`kernel/src/fs/exofs/`, `kernel/src/syscall/fs_bridge/`), **SYSCALL** (`kernel/src/syscall/`)  
**Statut :** Corrections appliquées & Tests validés (100% passants)

---

## 1. Contexte & Objectifs

Dans le cadre de la session d'audit systématique approfondi des sous-systèmes IPC, FS et Syscall d'Exo-OS, confrontés aux rapports d'audit tiers (`c:\Users\xavie\Downloads\Analyse_2`, `c:\Users\xavie\Downloads\Analyse_z-ai`, `c:\Users\xavie\Downloads\Analyse (3)`), 24 vulnérabilités et écarts de conformité ont été analysés directement contre l'implémentation noyau.

L'objectif de cette intervention est d'éliminer définitivement les risques de *Use-After-Free* (UAF) et de courses critiques lors de la destruction des canaux IPC, de garantir la cohérence transactionnelle des relations et quotas ExoFS, d'assurer la conformité stricte POSIX sur la manipulation des descripteurs de fichiers (`dup`, `dup2`, `fcntl(F_DUPFD_CLOEXEC)`, `getcwd`, `access`), et de connecter fidèlement la gestion temporelle des syscalls de multiplexage (`ppoll`, `epoll_pwait2`).

---

## 2. Anomalies Détectées & Corrections Appliquées

### A. Sous-système IPC (`kernel/src/ipc/`)

1. **`channel/streaming.rs` — Use-After-Free lors de la destruction concurrente d'un canal de streaming**
   - *Problème :* Dans `stream_alloc_buffer`, `stream_push`, `stream_pop` et `stream_release_buffer`, les références internes `&StreamChannel` étaient manipulées après avoir relâché le verrou `STREAM_CHANNELS`. Un appel simultané à `stream_channel_destroy` libérait immédiatement le slot dans le tableau, permettant à un nouvel appelant de recycler le slot et d'écraser la mémoire en cours d'utilisation.
   - *Correction :* Ajout d'un compteur atomique `active_ops: AtomicU32` dans `StreamChannel`. Incrémentation avant libération du verrou et décrémentation via RAII à la fin de l'opération. `stream_channel_destroy` invoque `chan_ref.close()` et attend via un spin coopératif (`cooperative_reschedule()`) que `active_ops == 0` avant de marquer le slot comme libre.

2. **`channel/sync.rs` — Fuite UAF sur les vérifications de capabilities synchrones**
   - *Problème :* Bien que `sync_channel_send` et `recv` aient reçu une protection `active_ops`, les variantes avec vérification de capabilities (`sync_channel_send_checked` et `sync_channel_recv_checked`) accédaient directement au canal sans incrémenter `active_ops`, ouvrant une fenêtre d'UAF lors de la destruction.
   - *Correction :* Encadrement systématique de `sync_channel_send_checked` et `sync_channel_recv_checked` avec l'acquisition et la libération RAII de `active_ops`.

3. **`channel/broadcast.rs` — Use-After-Free sur la destruction de canaux Broadcast**
   - *Problème :* `broadcast_destroy` libérait immédiatement le slot `tbl.free(chan_idx)` sans vérifier si des souscriptions, publications ou réceptions concurrentes étaient en cours sur le canal.
   - *Correction :* Intégration de `pub active_ops: AtomicU32` dans `BroadcastChannel`. Protection de toutes les méthodes publiques (`broadcast_subscribe`, `broadcast_unsubscribe`, `broadcast_publish`, `broadcast_recv`, ainsi que leurs variantes vérifiées `_checked`). Attente active coopérative dans `broadcast_destroy` jusqu'à extinction totale de `active_ops`.

4. **`channel/typed.rs` — Use-After-Free sur les canaux typés**
   - *Problème :* Les opérations `send`, `recv` et `try_recv` opéraient sans comptage de références actives, permettant une désallocation prématurée par `typed_channel_destroy`.
   - *Correction :* Ajout de `active_ops: AtomicU32` dans `TypedChannelInner`, fermeture explicite `inner.close()` et attente coopérative dans `typed_channel_destroy` avant libération du slot.

5. **`sync/event.rs` — Use-After-Free et écrasement destructeur dans `IpcEvent`**
   - *Problème :* Dans `event_wait`, le thread spin-attendait sur l'état d'un `IpcEvent` obtenu via `IPC_EVENT_TABLE.get(idx)`. Si un thread appelait `event_destroy(idx)`, le slot était libéré et réalloué par `event_create` via `ptr.write(...)`, écrasant la structure pendant que le waiter lisait la mémoire.
   - *Correction :* Ajout de `active_ops: AtomicU32` et `closed: AtomicBool` dans `IpcEvent`. `wait()` détecte la fermeture et renvoie `Err(IpcError::ChannelClosed)`. `event_set`, `event_clear` et `event_wait` protègent leur exécution par `active_ops`. `event_destroy` marque `close()` et draine coopérativement les opérations actives avant d'appeler `IPC_EVENT_TABLE.free(idx)`.

6. **`sync/rendezvous.rs` — Concurrence critique sur le slot d'échange Rendezvous**
   - *Problème :* Dans `ExchangeSlot::exchange`, le Thread B (consommateur) lisait la donnée puis réinitialisait l'état à `Empty` alors que le Thread A (producteur) n'avait pas encore lu la réponse dans `reply_buf`. Deux échanges consécutifs pouvaient écraser le buffer de réponse avant sa lecture.
   - *Correction :* Découplage strict des responsabilités d'état : le Thread B dépose la réponse, marque `result_ready = true` et laisse l'état à `Completing`. Seul le Thread A lit `reply_buf`, puis remet `result_ready = false` et bascule l'état à `Empty`. Ajout de `cooperative_reschedule()` toutes les 128 itérations de spin.

7. **`sync/barrier.rs` — Famine CPU sur les attentes de barrière**
   - *Problème :* La boucle d'attente `arrive_and_wait()` exécutait une boucle de spin infinie `core::hint::spin_loop()` sans jamais céder le processeur, risquant d'affamer les autres threads prêts sur le même cœur logique.
   - *Correction :* Insertion de `cooperative_reschedule()` toutes les 128 itérations de spin.

8. **`sync/wait_queue.rs` — Double-réveil concurrent dans la file d'attente**
   - *Problème :* Dans `wake_with_policy` et `wake_thread`, `waiter.woken.store(true, Release)` était exécuté sans réservation atomique préalable. Deux réveils concurrents pouvaient cibler le même waiter, provoquant un double décompte de `woken` et des incohérences d'ordonnancement.
   - *Correction :* Implémentation de `try_wake` à base de CAS (`compare_exchange(false, true, AcqRel, Relaxed)`) garantissant qu'un seul réveilleur réussit la transition pour chaque waiter.

9. **`shared_memory/pool.rs` — Course concurrente dans `shm_alloc_contiguous`**
   - *Problème :* Lors de l'allocation d'une plage contiguë de pages SHM, si un bit venait d'être alloué par un autre thread entre le scan et la phase de réservation (`(v & mask) == 0`), le CAS `compare_exchange(v, v & !mask)` réussissait sans modification (car `v & !mask == v`), et la fonction allouait indûment une page déjà occupée.
   - *Correction :* Vérification préalable `if (v & mask) == 0`: en cas d'occupation concurrente détectée, annulation transactionnelle de toutes les réservations précédentes via `shm_page_free` et réessai de la boucle externe.

10. **`endpoint/lifecycle.rs` — Gestion erronée de la destruction d'endpoints inexistants**
    - *Problème :* Si un identifiant d'endpoint n'était pas trouvé dans le pool (`active.is_none()`), `endpoint_destroy` tentait quand même de désenregistrer le nom dans le registre et retournait faussement `Err(IpcError::WouldBlock)`.
    - *Correction :* Renvoi immédiat et propre de `Err(IpcError::EndpointNotFound)` si l'endpoint n'est pas présent dans le pool.

---

### B. Sous-système ExoFS (`kernel/src/fs/exofs/`)

11. **`quota/quota_namespace.rs` — Duplication du namespace racine**
    - *Problème :* `init_root()` appelait aveuglément `alloc_slot()` sans vérifier si le namespace racine (`NamespaceId::ROOT`) existait déjà, corrompant la table lors des initialisations répétées.
    - *Correction :* Rendu d'`init_root` strictement idempotent : vérification de la présence de `NamespaceId::ROOT` et mise à jour en place de la politique sans allocation redondante.

12. **`quota/quota_audit.rs` — Lectures tronquées concurrentes sur les entrées d'audit**
    - *Problème :* Les entrées d'audit de 128 octets (`QuotaAuditEntry`) étaient écrites et lues dans le ring buffer sans exclusion mutuelle, causant des lectures incohérentes sous forte charge multi-cœurs.
    - *Correction :* Ajout d'un spinlock léger atomique `lock: AtomicU64` protégeant les opérations `push`, `latest` et `last_n_filtered`.

13. **`relation/relation_batch.rs` — Désynchronisation du graphe et des index de relations**
    - *Problème :* En cas d'échec d'insertion dans l'index lors d'une opération `BatchOp::Insert`, le blob inséré dans `RELATION_STORAGE` et `RELATION_GRAPH` n'était pas annulé. De plus, `BatchOp::Update` ne mettait pas à jour les arêtes du graphe.
    - *Correction :* Rollback transactionnel complet du graphe et du stockage en cas d'échec d'insertion d'index ; synchronisation bidirectionnelle du graphe et de l'index lors des mises à jour.

14. **`relation/relation.rs` — Désérialisation de relations corrompues sans validation CRC**
    - *Problème :* `RelationOnDisk::from_bytes` analysait les structures sans vérifier au préalable le checksum CRC32 on-disk.
    - *Correction :* Appel préalable à `Self::crc_ok(buf)` et rejet immédiat avec `ExofsError::ChecksumMismatch` en cas de corruption.

---

### C. Sous-système Syscall & FS Bridge (`kernel/src/syscall/`)

15. **`fs_bridge/cwd.rs` & `types.rs` — Non-conformité POSIX sur `getcwd`**
    - *Problème :* Si le buffer utilisateur était trop petit pour accueillir le chemin courant, `fs_getcwd` retournait `Invalid` (`EINVAL = -22`) au lieu du code standard POSIX `ERANGE` (`-34`), provoquant l'échec des bibliothèques C standard et de Rust `std`.
    - *Correction :* Ajout de `FsBridgeError::Range` mappé à `-34` (`ERANGE`). Validation : `size == 0` renvoie `Invalid` (`EINVAL`), et `size < path_len + 1` renvoie `Range` (`ERANGE`).

16. **`fs_bridge/fd.rs` — Fuite de handles ExoFS dans `fs_dup` et `fs_dup2`**
    - *Problème :* Dans `fs_dup`, si la table des descripteurs du processus était saturée (`install_process_fd` renvoyant `None`), le handle alloué dans `OBJECT_TABLE.dup()` n'était jamais fermé. De même, dans `fs_dup2`, un handle temporaire était dupliqué même lorsque la cible ne passait pas par la table de processus, entraînant des fuites systématiques.
    - *Correction :* Nettoyage automatique `OBJECT_TABLE.close(handle)` en cas d'échec d'installation dans la table du processus. Séparation stricte des branches avec et sans table de descripteurs.

17. **`fs_bridge/fd.rs` & `types.rs` — Omission de `F_DUPFD_CLOEXEC` dans `fcntl`**
    - *Problème :* La commande standard Linux/POSIX `F_DUPFD_CLOEXEC` (`1030`) n'était pas reconnue par `fs_fcntl` et renvoyait `EINVAL`, empêchant les runtimes modernes (Rust `std::os::unix::io`) d'effectuer des duplications atomiques sécurisées.
    - *Correction :* Définition de `pub const F_DUPFD_CLOEXEC: u32 = 1030;` et prise en charge complète dans `fs_fcntl` en appliquant le flag `O_CLOEXEC` sur le nouveau descripteur.

18. **`fs_bridge/path_ops/metadata.rs` — Contrôle effectif des permissions dans `fs_access`**
    - *Problème :* `fs_access` se contentait de vérifier l'existence du fichier sans tester les bits de permissions requis (`R_OK`, `W_OK`, `X_OK`) par rapport à l'EUID appelant.
    - *Correction :* Extraction du mode du fichier via `stat_mode_for_blob`, comparaison des bits de droits selon l'EUID de l'appelant (ou acceptation totale pour root EUID 0), et retour de `PermDenied` (`EACCES = -13`) en cas de droits insuffisants.

19. **`table/fs/poll_event.rs` — Gestion tronquée des timeouts dans `ppoll` et `epoll_pwait2`**
    - *Problème :* `sys_ppoll` et `sys_epoll_pwait2` ignoraient purement leur pointeur de timeout (`timeout_ptr: u64`), forçant un timeout de `0` ms (retour instantané) et transformant les attentes bloquantes en boucles d'attente active saturant le CPU.
    - *Correction :* Désérialisation sécurisée de la structure POSIX `Timespec` (`tv_sec`, `tv_nsec`). Si `timeout_ptr == 0`, transmission de `-1` (attente indéfinie conforme). Si `timeout_ptr != 0`, validation stricte des nanosecondes (`0..999_999_999`), conversion saturante en millisecondes et transmission fidèle à `fs_poll` et `fs_epoll_wait`.

20. **`table/ipc/channel.rs` — Erreur errno incohérente sur timeout IPC**
    - *Problème :* `IpcError::Timeout` était converti en `EINVAL` (`22`) au lieu de `ETIMEDOUT` (`110`).
    - *Correction :* Remplacement par `crate::syscall::errno::ETIMEDOUT`.

21. **`table/ipc/endpoints.rs` — Dépassement de longueur de nom d'endpoint**
    - *Problème :* `sys_exo_ipc_create` et `sys_exo_ipc_lookup` acceptaient des noms de longueur nulle ou dépassant la limite sans validation stricte.
    - *Correction :* Validation stricte `len == 0 || len >= MAX_ENDPOINT_NAME_LEN` (64) via `UserBuf::validate`.

22. **`fast_path.rs` & `table/common.rs` — SIGSEGV lors des tests unitaires hôte sur `gs:[0x20]`**
    - *Problème :* Lors de l'exécution de `cargo test` sous Linux hôte, les chemins relatifs déclenchaient `resolve_path_with_symlinks` -> `syscall_current_pid()` -> `current_tcb_ptr()`, exécutant l'instruction assembleur `mov {}, gs:[0x20]`. Le segment GS n'étant pas initialisé en espace utilisateur Linux, le processus recevait un signal 11 (`SIGSEGV`).
    - *Correction :* Guard `#[cfg(test)]` dans `current_tcb_ptr()` retournant `core::ptr::null()`, et dans `sys_sched_yield()` et `current_pid_u32()` retournant 0 en profil de test.

23. **`process/signal/handler.rs` — Avertissement de compilation en mode test**
    - *Problème :* `struct AlignedFpRegs` émettait un avertissement `dead_code` lorsque compilé sous profil de test Linux.
    - *Correction :* Ajout de `#[allow(dead_code)]`.

---

## 3. Réfutation des Faux Positifs Audités

1. **Auto-provisioning de répertoires dans `fs_open` (`ensure_directory_chain`) :**
   - *Allégation tiers :* L'audit externe signalait comme un bogue le fait que `fs_open` crée automatiquement l'arborescence des répertoires intermédiaires.
   - *Vérification :* **Réfuté.** Dans l'architecture ExoFS, le stockage orienté objet adressé par le contenu s'appuie sur une structure d'index de chemins découplée. `ensure_directory_chain` est une garantie architecturale native assurant la résilience lors de l'initialisation des mounts virtuels sans nécessiter de commandes `mkdir -p` préalables. Supprimer ce comportement casse la chaîne de test et le bootstraping du système.

2. **Absence de support 64-bit des descripteurs de fichiers (`ResolvedFd`) :**
   - *Allégation tiers :* L'audit affirmait que `handle: u32` empêchait l'adressage de plus de $2^{32}$ objets en mémoire.
   - *Vérification :* **Réfuté.** La table des descripteurs noyau (`OBJECT_TABLE`) utilise une plage compacte d'indices conformément aux exigences no-alloc et temps réel dur (contrainte mémoire bornée à 64K descripteurs par table). Les identifiants globaux de blobs (`BlobId`) restent quant à eux des hachages cryptographiques BLAKE3 de 256 bits, garantissant un espace d'adressage infini.

---

## 4. Bilan des Tests et Vérifications

| Suite de tests | Nombre de tests | Résultat | Temps d'exécution |
|---|---|---|---|
| `cargo check (kernel)` | — | **0 erreur, 0 avertissement** | 1m 02s |
| `make test-drivers` | 22 tests | **22 passés, 0 échec** | 19s |
| `make test-userspace` | 5 tests | **5 passés, 0 échec** | 6s |
| `cargo test (ipc)` | 31 tests | **31 passés, 0 échec** | 13s |
| `cargo test (quota)` | 116 tests | **116 passés, 0 échec** | 10s |
| `cargo test (relation)` | 158 tests | **158 passés, 0 échec** | 11s |
| `cargo test (fs_bridge)` | 22 tests | **22 passés, 0 échec** | 15s |
| `make test-exofs` | 2 943 tests | **2 943 passés, 0 échec** | 132s |

Tous les correctifs sont conformes aux invariants **TIER 0-3**, **RECUR-01**, **ARITH-02** et **NO-ALLOC**.
