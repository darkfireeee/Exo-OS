# Rapport d'Audit Approfondi N°45 — IPC, Scheduler, Syscall & ExoFS

**Date d'audit :** 2026-10-29  
**Périmètre analysé :** Rapports externes `Analyse_2`, `Analyse_z-ai`, `Analyse (3)`  
**Modules ciblés :**
1. **IPC** (`kernel/src/ipc/`, `kernel/src/memory/utils/futex_table.rs`, canaux raw/sync/mpmc/broadcast/streaming/typed, rings SPSC/MPMC, futex)
2. **SCHEDULER** (`kernel/src/scheduler/`, `kernel/src/arch/x86_64/sched_iface.rs`, P-states, fréquence, context switch, runqueue)
3. **FS** (`kernel/src/fs/exofs/`, cache policy/metadata/path, direct IO, IO batch, recovery fsck/epoch replay)
4. **SYSCALL** (`kernel/src/syscall/`, handlers signal/tgkill, compat Linux/POSIX, fs_bridge CWD et résolution de chemins)

---

## 1. Synthèse des Corrections Appliquées (21 Fichiers)

| N° | Fichier Modifié | Sous-système | Gravité | Description du Bogue & Solution Appliquée |
|---|---|---|---|---|
| 1 | `kernel/src/arch/x86_64/sched_iface.rs` | Scheduler / Arch | MAJEUR | **AMD `#GP` sur P-State MSR :** `arch_set_cpu_pstate` écrivait dans `MSR_IA32_PERF_CTL` sans vérifier le vendeur CPU. Sur processeurs AMD ou hyperviseurs tiers, cela déclenchait une exception `#GP(0)`. Ajout de la garde `cpu_features().vendor == CpuVendor::Intel`. |
| 2 | `kernel/src/fs/exofs/cache/cache_policy.rs` | ExoFS / Cache | MAJEUR | **Inversion des Watermarks de Cache :** `low_watermark` était codé en dur à 80 % alors que `high_watermark` en profil contraint (`low_memory()`, min_free_ratio=30) valait 70 %. Le seuil bas étant supérieur au seuil haut, `bytes_to_free()` renvoyait 0 et le cache bouclait indéfiniment sous pression. Calcul dynamique : `low_pct = high_pct.saturating_sub(15).max(50)`. |
| 3 | `kernel/src/fs/exofs/cache/metadata_cache.rs` | ExoFS / Cache | MAJEUR | **Éviction d'Entrées Dirty :** `evict_one` (LRU) évictait la plus ancienne entrée sans vérifier si `dirty == true`, provoquant une perte silencieuse de métadonnées non écrites sur disque. Filtrage ajouté : `filter(\|(_, v)\| !v.dirty)`. |
| 4 | `kernel/src/fs/exofs/cache/path_cache.rs` | ExoFS / Cache | MAJEUR | **Corruption des Chemins Non-ASCII :** `insert` construisait la chaîne avec `s.push(b as char)`, élargissant chaque octet ≥ 0x80 en point de code Unicode Latin-1 ré-encodé sur 2 octets en UTF-8. 0 % de hit pour les chemins accentués/internationaux. Remplacé par `core::str::from_utf8(path)`. Test unitaire ajouté. |
| 5 | `kernel/src/fs/exofs/io/direct_io.rs` | ExoFS / IO | MAJEUR | **Panique et Troncature Direct IO :** `DirectIoBuffer` stocke des `AlignedBlock512` fixes de 512 octets. En mode `BlockSize::B4096`, `read_to_slice` et `write_from_slice` provoquaient un dépassement de tranche (`index out of range`), et `read_aligned`/`write_aligned` tronquaient 3,5 Kio par bloc. Guarding strict des tailles de blocs non-512 via `InvalidArgument`. |
| 6 | `kernel/src/fs/exofs/io/io_batch.rs` | ExoFS / IO | MAJEUR | **Données de Lecture Batch Jetées :** `execute()` ignorait le premier élément du tuple `store.read(...)` (`Ok((_, bytes))`), rendant l'API de batch read incapable de renvoyer le contenu lu. Ajout de `read_data: Vec<(usize, Vec<u8>)>` dans `BatchResult` et méthode `add_read_ok`. |
| 7 | `kernel/src/fs/exofs/recovery/epoch_replay.rs` | ExoFS / Recovery | MAJEUR | **LBA sans Borne & Flush Ignorés :** Dans `replay_epoch`, `rec.data_lba` n'était pas borné par `device.total_blocks()`, risquant d'écraser des zones critiques si un journal était corrompu. De plus, `device.flush()` voyait ses erreurs ignorées. Ajout de la validation de borne `data_lba + blocks_needed <= total_blocks` et propagation des erreurs de flush. |
| 8 | `kernel/src/fs/exofs/recovery/fsck_repair.rs` | ExoFS / Recovery | CRITIQUE | **5 Réparations Corrompant les Structures Disque :**<br>• `rebuild_blob_header` : layout décalé (hash écrit sur `data_len` et `data_lba`), aligné sur `BlobHeaderDisk`.<br>• `clear_slot_dirty` : modifiait `slot_id` (octet 9) au lieu du bit 0 de `flags` (octet 10) et ne recalculait pas `header_hash`.<br>• `rebuild_alloc_table_header` : utilisait le magic erroné `0x424C54414F465845` au lieu de `ALLOC_TABLE_MAGIC` (`0x54414B4C424F5845`).<br>• `mark_snapshot_deleted` : activait le bit sans recalculer le Blake3 header hash.<br>• `fix_ref_count` : écrivait à l'offset 48 (sur `data_len`) au lieu de l'offset 12 de `BlobHeaderDisk`. |
| 9 | `kernel/src/ipc/channel/broadcast.rs` | IPC / Broadcast | MAJEUR | **Course sur l'Allocation d'Abonnés :** `subscribe` et `unsubscribe` testaient `is_active()` puis appelaient `activate()` sans atomique CAS, permettant à deux threads de revendiquer le même slot et corrompant `subscriber_count`. Implémentation de `try_activate` et `try_deactivate` via CAS atomique. |
| 10 | `kernel/src/ipc/channel/mpmc.rs` | IPC / MPMC | MINEUR | **Underflow Atomique `DropOldest` :** `self.pending.fetch_sub(1)` était appelé inconditionnellement même si `pop_into` échouait en raison d'une course concurrente. Conditionné au succès de `pop_into`. |
| 11 | `kernel/src/ipc/channel/raw.rs` | IPC / Raw | MINEUR | **CPU Burn sans Reschedule dans `recv_raw` :** La boucle de spin tournait 1 000 000 d'itérations en verrouillant l'anneau sans jamais relâcher le CPU. Ajout d'un `cooperative_reschedule()` tous les 128 tours comme dans `send_raw`. |
| 12 | `kernel/src/ipc/channel/streaming.rs` | IPC / Streaming | MAJEUR | **Absence de Vérification dans `make_ref` :** `make_ref` n'assurait pas que le buffer était alloué (`!buf.is_available()`) ni que `data_len <= buf.zc.capacity`, exposant à un UAF ou un buffer overflow. Vérifications ajoutées avec retour `None`. |
| 13 | `kernel/src/ipc/channel/sync.rs` | IPC / Sync | MAJEUR | **Destruction de Canal sans Réveil & Troncature Silencieuse :**<br>• `sync_channel_destroy` libérait le slot sans appeler `chan.close()`, laissant les threads bloqués indéfiniment.<br>• `send_zerocopy` n'attendait pas que l'état redevienne `Idle` si non-NOWAIT.<br>• `recv` tronquait silencieusement `len.min(buf.len())` et acquittait l'émetteur. Maintenant retourne `Err(MessageTooLarge)` sans consommer. |
| 14 | `kernel/src/ipc/channel/typed.rs` | IPC / Typed | MAJEUR | **Incohérence de Taille `MAX_TYPED_VALUE_SIZE` :** Déclaré à 512 alors que le `SpscRing` sous-jacent a `MAX_MSG_SIZE = 240`. Les types entre 241 et 512 passaient l'assert compile-time mais échouaient systématiquement à l'exécution avec `MessageTooLarge`. Constante ramenée à `MAX_MSG_SIZE` (240). |
| 15 | `kernel/src/ipc/ring/mpmc.rs` | IPC / Ring | MAJEUR | **Perte de Message sur Buffer Trop Petit :** `pop_into` avançait `tail` par CAS avant de vérifier `dst.len() >= slot.header.len`. Si le buffer était trop court, il libérait le slot et perdait le message définitivement. Vérification déplacée avant le CAS de `tail`. |
| 16 | `kernel/src/memory/utils/futex_table.rs` | IPC / Futex | MAJEUR | **Deadlock Requeue dans le Même Bucket :** Lorsque `src_addr` et `dst_addr` se projetaient dans le même bucket de hash (ex: `pthread_cond` et son mutex), `futex_requeue` abandonnait le requeue (« Simplification: wake seulement »). Les waiters restaient rattachés à `src_addr` et n'étaient jamais réveillés par `futex_wake(dst_addr)`. Implémentation de `requeue_inplace(&mut self, src, dst, max_requeue)`. |
| 17 | `kernel/src/syscall/compat/linux.rs` | Syscall / Compat | MAJEUR | **Numéro 135 Bloqué par Erreur :** Dans `LINUX_REMOVED`, le commentaire indiquait `135 // uselib`. Sur x86_64 Linux, 134 est `uselib` et 135 est `sys_personality`. Corrigé 134 dans `LINUX_REMOVED` pour débloquer `personality`. |
| 18 | `kernel/src/syscall/compat/posix.rs` | Syscall / POSIX | MINEUR | **Handler `sys_personality` (135) :** Implémentation du handler retournant 0 (`PER_LINUX`) et câblage dans `get_posix_handler` pour le support des chargeurs et runtimes libc. |
| 19 | `kernel/src/syscall/fs_bridge/cwd.rs` | Syscall / FS Bridge | MAJEUR | **Exposition du CWD Actif :** Ajout de la fonction `current_cwd_for_pid(pid)` permettant d'extraire le chemin courant d'un processus sous verrouillage de `CWD_LOCK`. |
| 20 | `kernel/src/syscall/fs_bridge/path_ops/resolution.rs` | Syscall / FS Bridge | MAJEUR | **Résolution Réelle des Chemins Relatifs via CWD :** Auparavant, tout chemin relatif était ancré directement sur `/`, rendant `chdir` purement cosmétique. `resolve_path_with_symlinks` préfixe désormais le CWD du processus appelant pour les chemins relatifs. |
| 21 | `kernel/src/syscall/handlers/signal.rs` | Syscall / Signal | MAJEUR | **`sys_tgkill` Multi-Thread Réel :** `sys_tgkill` ignorait le paramètre `tid` et ciblait toujours `pcb.main_thread_ptr()`. Désormais, le thread correspondant à `tid` est recherché dans `pcb.thread_slots` (avec repli sur le main thread si `tid == tgid` ou `tid == 0`). |

---

## 2. Réfutations Techniques des Hallucinations d'Audit

Plusieurs affirmations des rapports externes ont été vérifiées et réfutées avec le code source à l'appui :
1. **`current_creds()` retourne fail-open `Credentials::root()` :**  
   *Réfutation :* Le code en production contenait déjà `None => Credentials::nobody()` (uid=65534, gid=65534, ppid=1) pour tout PID non-nul introuvable.
2. **`fsck_phase1.rs` débordement de pile sur buffer 64B :**  
   *Réfutation :* L409 et L442 utilisent déjà `device.read_block(..., &mut sb_block[..sb_blk])` avec `sb_block: [0u8; SB_READ_BUF]` (4096 octets), puis copient 64 octets dans le buffer cible.
3. **`spsc.rs` `pop_into` perd le message sur buffer trop petit :**  
   *Réfutation :* `kernel/src/ipc/ring/spsc.rs` lignes 209-213 vérifiait déjà `ln > dst.len()` et retournait `Err(MessageTooLarge)` sans avancer `pos` ni appeler `cell.store_seq`. Le bogue n'existait que dans `mpmc.rs` (corrigé ici).

---

## 3. Validation et Résultats de Tests

Tous les tests unitaires, d'intégration, les pilotes matériels et la compilation des serveurs Ring 1 s'exécutent avec un taux de réussite de 100 % sous WSL :
- **`cargo check --manifest-path kernel/Cargo.toml`** : 0 erreur, 0 avertissement.
- **`make test-drivers`** : 22/22 tests réussis (`exo_ps2_input`, `exo_tty`, `exo_vga_text`, `exo_virtio_blk`).
- **`make test-userspace`** : 100 % de réussite (`exo_coreutils`, `exosh`, `libexo`).
- **`make test-exofs`** : 2 943 tests passés sur 2 943 en 83,49 secondes.
- **`make build-rootfs-binaries`** : Staging propre et génération des 28 serveurs Ring 1 dans `target/exofs-rootfs`.
