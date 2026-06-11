# Résolution des audits FS (AUDIT-EXOFS-COMPLET + AUDIT-V020-FS-IPC-SCHED-DATAPATH)

**Traité le :** 2026-06-11 · **Base :** HEAD `601f445` · **Build/test :** WSL
**Méthode :** vérification du code réel avant toute correction (audits = analyse
statique sans compilateur ⇒ faux positifs confirmés sur plusieurs points).

---

## Bilan central

Le diagnostic de l'audit EXOFS est exact : le moteur transactionnel et le pipeline
de stockage existent et sont corrects, mais **débranchés**. Le blocage n°1 —
`commit_epoch` jamais déclenché en fonctionnement normal (CORE-1) — est **corrigé**.
Les écritures atteignent désormais le disque via un commit d'epoch transactionnel
(atomicité + recovery réels). Les autres points sont soit corrigés, soit vérifiés
comme déjà sains, soit identifiés comme suggestions d'audit à régression.

`make test` : **3077 passed; 0 failed** après tous les changements.

---

## CORE (jonctions manquantes)

### EXOFS-CORE-1 · ✅ CORRIGÉ — commit_epoch branché sur le chemin chaud
`commit_epoch` n'était déclenché qu'au démontage ⇒ blobs bruts isolés sur disque,
sans EpochRecord (pas d'atomicité, recovery sans objet).
**Fix (FIX-EXOFS-CORE-1) :** nouveau point d'entrée kernel
`epoch_commit::commit_current_epoch()` (réutilise `do_commit`, même chemin que
shutdown : flush blobs dirty → journal → EpochRoot → EpochRecord + 3 barrières
NVMe). Branché sur :
- le writeback thread (`mod.rs::exofs_writeback_dirty`) — remplace la persistance
  brute isolée ;
- `sync()` (`fs_bridge::fs_sync`) ;
- `fsync()` (`fs_bridge::fs_fsync`, `data_only==false`).
`CommitInProgress` est absorbé proprement (commit concourant). Validé par `make test`.

### EXOFS-CORE-3 · ✅ VÉRIFIÉ — PathIndex déjà on-disk, survit au reboot
Tracé complet de la résolution de chemin :
`path → blob_id_for_path (Blake3 déterministe) → blob répertoire`.
- Le PathIndex EST persisté sur disque (les répertoires sont des blobs ;
  `store_path_index` → `mark_dirty` ⇒ committé via CORE-1).
- `load_path_index` → `snapshot_blob` retombe sur le disque
  (`object_store::load_blob_data_if_available`) en cas de cache-miss.
- Le catalogue BlobId→LBA est persisté **inconditionnellement** après chaque
  écriture (`persist_blob_data_if_disk` → `persist_catalog_to_global_disk`) et
  rechargé au boot (`ensure_catalog_loaded`).
**Aucun changement nécessaire** : l'arborescence survit au reboot dès lors que
CORE-1 fait atteindre le disque aux blobs répertoire dirty (désormais le cas).

### EXOFS-CORE-2 · ⏳ DOCUMENTÉ — pipeline stockage (effort dédié requis)
Constat audit exact : `persist_blob_data_if_disk` écrit brut ; BlobWriter
(dédup→compress→crypto→checksum) est inerte ; **`ObjectKind::Secret` non chiffré
sur disque** (viole LOBJ-03/SEC-07).
**Pourquoi non corrigé dans cette passe (risque de corruption du FS validé) :**
1. `persist_blob_data_if_disk(blob_id, data, sync)` ne connaît PAS l'ObjectKind
   (perdu sur le chemin write→cache→writeback→persist) — router via BlobWriter
   exige de propager le kind ou de consulter l'OBJECT_TABLE à la persistance.
2. Changer le format on-disk (raw → compressé/chiffré/checksummé) casserait
   l'image `exofs-root.img` validée (blobs bruts) ET le chemin de lecture
   (`load_blob_data_if_available` lit du raw) sans versionnement de format.
**Plan sûr (à mener en passe dédiée) :**
- Ajouter un marqueur/version de format par blob persisté (raw=0, pipeline=1) ;
  le reader détecte et décompresse/déchiffre/vérifie en conséquence (compat
  ascendante avec les blobs raw existants).
- Threader l'ObjectKind jusqu'à la persistance (ou lookup OBJECT_TABLE) pour
  rendre le chiffrement XChaCha20 **obligatoire** pour `Secret` (refus d'écriture
  raw d'un Secret).
- Brancher `checksum_reader`/`DecompressReader` symétriquement à la lecture.
- Migration : régénérer `exofs-root.img` via `tools/exofs_mkroot` au nouveau format.

---

## ROBUSTESSE

### EXOFS-ROB-1 · ✅ CORRIGÉ — commit refusé si flush NVMe non enregistré
`commit_durable_epoch_if_disk` retourne `NvmeFlushFailed` si un disque est présent
mais qu'aucun hook de flush NVMe n'est enregistré (au lieu d'exécuter les 3
barrières en no-op = fausse durabilité, EPOCH-02). Le chemin dev-sans-disque reste
court-circuité par `has_global_disk()==false`.

### EXOFS-ROB-2 · ⚪ FAUX POSITIF — désérialisation disque déjà sûre
Les sites cités sont tous hors production :
- `key_storage.rs:927,944,953` → dans `#[cfg(kani)] mod kani_proofs` (preuves Kani
  avec préconditions `kani::assume` ⇒ l'expect ne peut prouvablement pas paniquer).
- `blob_cache.rs:916,929` → dans des `#[test]`.
Les vraies fonctions de production `key_kind_from_u8`/`slot_state_from_u8`
retournent déjà `ExofsResult` (propagation, pas de panic). Scan outillé
(`scan_unsafe_patterns.py`) : **0** unwrap/expect production dans tout `fs/exofs`.
**Aucun changement nécessaire.**

### EXOFS-ROB-3 · ✅ partiel + ⚪ faux positif
- `io/writer.rs:178` : ✅ `Vec::with_capacity(len)` était **aussitôt écrasé par
  `mem::swap`** — allocation infaillible gaspillée. Remplacé par `Vec::new()`
  (+ suppression d'une boucle while vide morte).
- `audit_rotation.rs:444` : ⚪ dans un `#[test]`.
- `cache_warming.rs:129`, `path_index.rs:620` : allocations bornées/minuscules
  (148 octets fixes) dans des fonctions retournant `Vec` (pas de `Result` à
  propager). Churn de signature non justifié pour un P2. Laissés tels quels.

### EXOFS-ROB-4 · ✅ CORRIGÉ (volet fsync) — durabilité données + métadonnées
`fs_fsync` durabilise désormais les données ET, pour `fsync` (≠ `fdatasync`),
scelle un epoch pour les métadonnées (PathIndex, table d'objets) via CORE-1.
`data_only` est respecté (fdatasync = données seules).

---

## DATAPATH (audit FS-IPC-SCHED)

### Z1 / Z2 · ⚪ SUGGESTION À RÉGRESSION — design copiant intentionnel
L'audit propose de remplacer `read_at→Vec` par `get()→Arc<[u8]>`. **Régression :**
- Le cache est **paginé** (`pages: Vec<Option<Arc<[u8;PAGE]>>>`). `get()` →
  `materialize_snapshot()` matérialise le blob **entier** en Arc contigu : pour une
  lecture partielle d'un gros fichier (cas commun : 4 KiB d'un fichier de 100 MiB),
  cela allouerait tout le fichier. `read_at(offset,count)` ne copie que la plage.
- Le `Vec` intermédiaire de `read_at`/`read_user_bytes` existe pour **libérer le
  verrou du cache AVANT `copy_to_user`** (qui peut fauter). Copier directement
  depuis les pages vers l'espace user tout en tenant le verrou casserait la
  sûreté du verrou (fault sous lock).
**Le design copiant actuel est correct et intentionnel.** Non modifié.

### F2 (read/write bloquants) / F3 (pipe O(n²)) · ⏳ DOCUMENTÉ — infra requise
- F2 : un `read()` sur pipe/socket vide retourne `WouldBlock`→EAGAIN sans tester
  `O_NONBLOCK` ni bloquer. Correct POSIX exige un blocage réel
  (`block_current_thread` + file d'attente + réveil sur écriture) — feature
  d'ordonnancement à part entière, risquée à câbler à la hâte sur le chemin chaud.
- F3 : les pipes recopient le tampon résiduel à chaque op (O(n²)) ; un vrai
  ring-buffer FIFO est nécessaire.
Ces deux points relèvent d'une passe « event-driven scheduler » dédiée (cf. aussi
Z4/Z5 de l'audit datapath : futex spin-poll, RING_SIZE IPC=16). Hors périmètre sûr
de cette passe FS.

### F1 (double chemin Ring0/Ring1) · ✅ CLARIFIÉ — design hybride voulu
L'audit EXOFS lui-même cadre : « ExoOS est hybride volontairement » (ExoFS en Ring0
pour des écritures rapides). vfs_server n'est PAS du code mort : `handle_request`
sert les ops POSIX comme front-end IPC relayant vers les syscalls ExoFS du kernel,
tandis que le kernel les sert aussi en direct (chemin rapide Ring0). Le commentaire
d'en-tête périmé (« ops 4..14 déléguées au kernel ») a été corrigé pour décrire le
design hybride réel. Aucun code fonctionnel supprimé.

### B1 (execve depuis ExoFS) · ✅ déjà correct (vérifié par l'audit)
Chargement zéro-copie cache via `read_blob_from_cache → Arc<[u8]>`. RAS.
### B2 (signature binaire) · ✅ CORRIGÉ — voir AUDIT-V020-RESOLUTION P1-1
Feature `strict_exec_signatures` déclarée (était fantôme).

---

## Validation
- `cargo check -p exo-os-kernel` OK · `-p exo-vfs-server` OK.
- `make test` : **3077 passed; 0 failed; 3 ignored** (durabilité transactionnelle
  CORE-1 incluse, aucune régression).
- Reste pour un FS « 100 % » : EXOFS-CORE-2 (pipeline + chiffrement Secret, passe
  dédiée avec versionnement de format) et F2/F3 (blocage POSIX réel, passe
  scheduler event-driven).
