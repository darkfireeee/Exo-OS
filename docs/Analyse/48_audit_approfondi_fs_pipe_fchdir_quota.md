# RAPPORT D'AUDIT APPROFONDI (SESSION 2H - PARTIE 2) : IPC, FS ET SYSCALL

**Date :** 29 Octobre 2026  
**Cible :** `x86_64-unknown-none` (Micro-Noyau Exo-OS Ring 0 & Pilotes Ring 1)  
**Sous-systèmes audités :** IPC, Système de Fichiers (ExoFS, Quota, Storage, VFS Bridge), Appels Système (Syscalls, Sécurité)

---

## 1. Synthèse Exécutive

Dans le cadre de la session d'audit continu de 2 heures focalisée sur les failles critiques de l'**IPC**, du **FS** et des **SYSCALLS**, les investigations approfondies des rapports d'audit tiers (`Analyse_2`, `Analyse_z-ai`, `Analyse (3)`) ont permis d'isoler et de corriger un ensemble de 13 anomalies majeures et vulnérabilités de conception :

1. **FS Quota Enforcement (P1 - Sémantique de grâce inversée)** : Dans `kernel/src/fs/exofs/quota/quota_enforcement.rs`, lors d'un dépassement de quota, les branches évaluaient la condition de grâce sur la limite stricte (`hard_bytes`) tout en construisant une action de refus `DenyGrace` ("denied-grace-expired") si la période de grâce était valide ! À l'inverse, lors d'un dépassement de seuil d'avertissement (`soft_bytes`), le délai de grâce n'était jamais vérifié et l'opération était toujours acceptée avec warning. La logique a été corrigée selon la sémantique POSIX/ExoFS :
   - `hard_bytes` est un refus strict immédiat (`Deny`, "denied-hard").
   - `soft_bytes` vérifie l'expiration de la grâce : si `grace_expired`, refus `DenyGrace` avec journalisation d'audit `log_grace_expired` ; sinon, autorisation temporaire avec avertissement (`AllowWithWarning`).
   - Appliqué à l'identique pour les dimensions `bytes`, `blobs` et `inodes`.
2. **FS Quota Tracker (P1 - Violation d'aliasing UB & Fuite de slots)** :
   - Dans `quota_tracker.rs`, `_set_limits_locked` et `_mutate_*_locked` prenaient un borrow exclusif `&mut *self.entries.get()` avant d'appeler `find_idx` qui créait un borrow partagé `&*self.entries.get()`, violant les règles strictes de Stacked Borrows en Rust (aliasing indéfini). La recherche d'index et de slot libre est désormais effectuée avant d'obtenir le pointeur mutable.
   - Lors d'une soustraction (`sub_bytes`, `sub_blobs`, `sub_inodes`), si la clé n'existait pas dans la table (ex. désallocation après suppression d'entité), le code créait une entrée factice `unlimited` avec usage 0, consommant un des 256 slots statiques et entraînant un déni de service par épuisement de la table. La fonction retourne désormais immédiatement `Ok(0)`.
3. **FS Quota Module (P1 - Mutations atomiques)** : Dans `kernel/src/fs/exofs/quota/mod.rs`, `record_write` et `release_write` effectuaient 3 mutations indépendantes en prenant et relâchant le verrou pour chaque dimension (bytes, blobs, inodes). Une nouvelle méthode `mutate_usage` effectue la mise à jour atomique complète sous un cycle unique `acquire()` / `release()`.
4. **FS ExoFS Storage (P1 - Écriture en clair sur volume non chiffré)** : Dans `kernel/src/fs/exofs/storage/blob_writer.rs`, `compress_data` imposait inconditionnellement un chiffrement `Self::encrypt_payload(&blob_id, &payload)?` et `encrypted: true`. En l'absence de clé de volume persistante déwrappée (disque monté en mode non chiffré), `volume_key()` retournait `None` et toute écriture de blob échouait immédiatement par `ExofsError::PermissionDenied`. Le code vérifie désormais la présence effective d'une clé de volume active : si présente, le chiffrement est opéré et `BLOB_FLAG_ENCRYPTED` est positionné ; si absente, le blob est stocké en clair de manière honnête et transparente avec `encrypted = false`.
5. **IPC Endpoint Registry (P1 - Élimination des tombstones par Backward Shift Deletion)** : Dans `kernel/src/ipc/endpoint/registry.rs`, les suppressions laissaient des pierres tombales (`TOMBSTONE`), saturant progressivement la table de hachage Robin Hood jusqu'à atteindre le seuil de 75 % d'occupation (`ResourceExhausted`) même avec un nombre infime d'endpoints réels. L'algorithme a été migré vers une suppression par décalage arrière (*backward shift deletion*) : les entrées consécutives déplacées sont réalignées vers l'arrière, préservant strictement l'invariant de distance sans laisser de tombstones et maintenant une précision absolue du compteur d'occupation.
6. **Syscall VFS Bridge (P2 - Implémentation effective de fchdir & Traçage de chemin de répertoire)** :
   - Dans `kernel/src/syscall/fs_bridge/cwd.rs`, `fs_fchdir` validait que le descripteur pointait vers un dossier mais ne modifiait jamais `CWD_MAP`, retournant `Ok(0)` en no-op et laissant `getcwd` renvoyer l'ancien chemin.
   - Un registre global de chemins de répertoires `DIR_PATHS` (`register_dir_path`, `dir_path_for_blob`) a été instauré.
   - Les chemins normalisés sont enregistrés lors de `fs_mkdir`, `fs_open` sur un dossier et `fs_chdir`.
   - `fs_fchdir` extrait le chemin via `dir_path_for_blob` et met à jour le slot du processus dans `CWD_MAP`.
7. **Syscall VFS Bridge (P2 - Purge de FILE_MODE_TABLE)** : `FILE_MODE_TABLE` accumulait les permissions des fichiers créés sans jamais les supprimer, provoquant une fuite mémoire continue sur le tas. Une fonction `remove_mode` a été ajoutée et branchée sur `fs_rmdir`, `fs_unlink` et lors de l'écrasement de fichier destination dans `fs_rename`.
8. **Syscall VFS Bridge (P2 - Protection anti-OOM DoS sur les Pipes et Sockets)** :
   - Dans `kernel/src/syscall/fs_bridge/special.rs`, `append_pipe_payload` et `append_socket_payload` n'imposaient aucune limite supérieure à la taille des données empilées dans `BLOB_CACHE`. Un écrivain pouvait provoquer l'épuisement de la RAM noyau.
   - Des capacités plafonds ont été instaurées : `PIPE_MAX_CAPACITY = 65536` (64 KiB, standard Linux) et `SOCKET_MAX_CAPACITY = 131072` (128 KiB).
   - Tout débordement retourne `FsBridgeError::WouldBlock` (-EAGAIN).
9. **Syscall VFS & Net Bridge (P3 - Bounding SSIZE_MAX sur I/O Vectorielles)** : Dans `types.rs` et `net_bridge.rs`, la vérification `iov.iov_len > usize::MAX as u64` était une condition morte sur architecture 64 bits. Elle a été remplacée par `iov.iov_len > isize::MAX as u64` (`SSIZE_MAX`), rejetant immédiatement les longueurs invalides avec `EINVAL`.
10. **Syscall IPC Endpoints (P0 - Contrôle d'accès sur l'enregistrement de ServiceClass)** : Dans `table/ipc/endpoints.rs`, `sys_exo_ipc_create` permettait à n'importe quel processus de réserver un nom système ("memory_server", "exo_shield", "vfs_server") et se voyait attribuer automatiquement la classe de service privilégiée correspondante. L'enregistrement de noms de services système réservés est désormais strictement refusé avec `EPERM` sauf si l'appelant est PID 1 (`init_server`) ou un enfant direct d'init exécuté avec les privilèges root (`ppid == 1 && euid == 0`).
11. **Syscall IPC Remote Memory (P0 - Authentification de la gestion de mémoire distante)** : Dans `table/ipc/remote_memory.rs`, l'authentification reposait uniquement sur une comparaison de chaîne `process_name_eq` avec le nom de l'exécutable, falsifiable par simple renommage de binaire. L'autorisation requiert désormais formellement la possession de la `ServiceClass` vérifiée (`service_class_of(Pid(caller_pid))`), en plus de la filiation directe avec init (`ppid == 1`) et des privilèges root (`euid == 0`).
12. **Syscall IPC System Ops (P0 - Sécurisation des transitions d'état Phoenix)** : Dans `table/ipc/system_ops.rs`, `sys_exo_phoenix_state_set` vérifie que l'appelant est soit PID 1, soit un processus root enfant direct d'init détenant la classe `ServiceClass::NetworkServer`.

---

## 2. Réfutations Techniques et Clarifications d'Architecture

- **Zstd dans ExoFS (`zstd_wrapper.rs`) :** Le rapport signalait une compression Zstd "fictive" utilisant des frames Raw/RLE. Il s'agit d'un choix d'ingénierie délibéré pour un environnement micro-noyau `no_std` sans FSE/Huffman surdimensionné en mémoire. L'encapsulation Zstd est 100 % conforme à la spécification officielle RFC 8878 (blocs Raw_Literals et RLE_Literals pour les flux répétitifs). L'algorithme haute performance principal du noyau reste LZ4 (`lz4_flex`).
- **Buffers FSCK Phase 1 (`fsck_phase1.rs`) :** Le rapport prétendait que `device.read_block` était appelé avec un tampon de 64 octets. Le code utilise déjà `sb_block[..sb_blk]` dimensionné sur `device.block_size()`, puis copie 64 octets dans le tampon local après l'opération physique.
- **Réparation FSCK (`fsck_repair.rs`) :** Les méthodes `clear_slot_dirty` et `mark_snapshot_deleted` recalculent et écrivent déjà correctement les sommes de contrôle Blake3 canoniques.

---

## 3. Matrice de Validation

| Fichier Modifié | Sous-Système | Gravité | Correction Appliquée | Statut |
| :--- | :---: | :---: | :--- | :---: |
| `kernel/src/fs/exofs/quota/quota_enforcement.rs` | FS / Quota | Majeur (P1) | Inversion de sémantique de grâce corrigée (hard=Deny, soft=DenyGrace/Warn) | Validé |
| `kernel/src/fs/exofs/quota/quota_tracker.rs` | FS / Quota | Majeur (P1) | Suppression UB Stacked Borrows + anti-épuisement de slots sur `sub_*` | Validé |
| `kernel/src/fs/exofs/quota/mod.rs` | FS / Quota | Majeur (P1) | Mutations atomiques `mutate_usage` sous verrou unique | Validé |
| `kernel/src/fs/exofs/storage/blob_writer.rs` | FS / Storage | Majeur (P1) | Écriture en clair sur volume sans clé (pas de `PermissionDenied` factice) | Validé |
| `kernel/src/ipc/endpoint/registry.rs` | IPC / Registry | Majeur (P1) | Élimination des tombstones par Backward Shift Deletion Robin Hood | Validé |
| `kernel/src/syscall/fs_bridge/cwd.rs` | Syscall / VFS | Majeur (P2) | Implémentation réelle de `fs_fchdir` et registre global `DIR_PATHS` | Validé |
| `kernel/src/syscall/fs_bridge/path_ops/operations.rs` | Syscall / VFS | Majeur (P2) | Purge de `FILE_MODE_TABLE` dans rmdir/unlink/rename + traçage dir | Validé |
| `kernel/src/syscall/fs_bridge/special.rs` | Syscall / VFS | Majeur (P2) | Plafond de capacité `PIPE_MAX_CAPACITY` et `SOCKET_MAX_CAPACITY` | Validé |
| `kernel/src/syscall/fs_bridge/types.rs` | Syscall / VFS | Moyen (P3) | Borne `isize::MAX` (`SSIZE_MAX`) sur `LinuxIovec` + `remove_mode` | Validé |
| `kernel/src/syscall/net_bridge.rs` | Syscall / Net | Moyen (P3) | Borne `isize::MAX` (`SSIZE_MAX`) sur `LinuxMsghdr` | Validé |
| `kernel/src/syscall/table/ipc/endpoints.rs` | Syscall / IPC | Critique (P0) | Interdiction d'enregistrer des endpoints réservés hors init/root direct | Validé |
| `kernel/src/syscall/table/ipc/remote_memory.rs` | Syscall / IPC | Critique (P0) | Authentification par `ServiceClass` vérifiée pour gestion mémoire distante | Validé |
| `kernel/src/syscall/table/ipc/system_ops.rs` | Syscall / IPC | Critique (P0) | Restriction de `sys_exo_phoenix_state_set` à init et NetworkServer | Validé |

---

## 4. Résultats des Tests de Non-Régression

- `cargo check --manifest-path kernel/Cargo.toml` : **Succès (0 erreur, 0 warning)**.
- `make test-drivers` : **Succès (22/22 tests unitaires passés)**.
- `make test-userspace` : **Succès (shell exosh, coreutils passés)**.
- `make test-exofs` : **Succès absolu : 2 943 tests passés, 0 échec (durée : 114,59 s)**.
