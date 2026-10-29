# Rapport d'Audit Approfondi #50 — IPC Channels, POSIX Open Flags, ExoFS Bridge & Inode Limits

**Date :** 29 octobre 2026  
**Périmètre :** IPC Channels (`sync.rs`, `raw.rs`, `async.rs`, `mpmc.rs`), ExoFS (`export`, `numa_placement`, `object_kind/config`, `object_kind/path_index`, `object_fd/flags`), Syscall FS Bridge (`file_ops.rs`, `types.rs`, `handlers/fd.rs`, `handlers/fs_posix.rs`).  
**Statut :** Résolu et validé par compilation et tests automatisés.

---

## 1. Synthèse des Vulnérabilités & Corrections

### A. IPC Sync Channel (`kernel/src/ipc/channel/sync.rs`)
- **Problème de concurrence sur l'émetteur :** Dans `SyncChannel::send` et `send_zerocopy`, l'émetteur observait l'état `Idle` puis écrivait directement dans `slot.data`. Deux émetteurs concurrents pouvaient simultanément observer `Idle` et écraser mutuellement leurs données.
- **Correction :** Ajout de l'état `SenderWriting = 1` dans `RendezVousState`. L'émetteur effectue une réservation atomique par CAS (`Idle -> SenderWriting`). En cas d'échec du CAS, un réessai coopératif est exécuté.
- **Use-After-Free sur `sync_channel_destroy` :** Lorsqu'une opération d'envoi ou de réception était en attente (spin ou blocage), un appel concurrent à `sync_channel_destroy` libérait immédiatement le slot dans `SYNC_CHANNELS`.
- **Correction :** Ajout d'un compteur `active_ops: AtomicU32` dans `SyncChannel`. `sync_channel_send` et `recv` incrémentent `active_ops` avant de déverrouiller et décrémentent via RAII à la fin de l'opération. `sync_channel_destroy` effectue un drain coopératif jusqu'à ce que `active_ops == 0` avant de marquer le slot comme libre.

### B. IPC Raw Mailbox (`kernel/src/ipc/channel/raw.rs`)
- **Race condition à la fermeture du canal :** Dans `mailbox_close`, le CAS réinitialisant `endpoint_id` à 0 s'exécutait avant l'acquisition du verrou `slot.ring.lock()`. Un émetteur concurrent pouvait franchir le test avant le CAS et tenter d'enfiler un message pendant ou après le reset de l'anneau.
- **Correction :** Acquisition préalable du verrou `slot.ring.lock()` avant d'exécuter le CAS et de réinitialiser l'anneau.

### C. IPC Asynchronous Channel (`kernel/src/ipc/channel/async.rs`)
- **Inversion de verrou & interblocage potentiel lors de l'appel aux wakers :** `fire_all()` appelait les pointeurs de fonction `wake_fn(data)` tout en détenant `SpinLock<WakerTable>`. Si le callback tentait d'enregistrer, de désenregistrer un waker ou d'interagir avec le canal, un auto-blocage sur spinlock survenait.
- **Correction :** Implémentation de `collect_active_wakers(&mut self)` qui extrait les callbacks et données opaques dans un tableau local de taille fixe (32 éléments) et marque les wakers déclenchés. Les callbacks sont ensuite invoqués en dehors de tout verrou spinlock.

### D. IPC MPMC Channel (`kernel/src/ipc/channel/mpmc.rs`)
- **Famine CPU sur spinlocks bloquants :** En politique `OverflowPolicy::Block` et en mode `recv()` bloquant, le thread bouclait jusqu'à 500 000 ou 2 000 000 d'itérations avec un simple `core::hint::spin_loop()` sans céder le processeur, empêchant le consommateur ou le producteur concurrent sur le même cœur d'avancer.
- **Correction :** Ajout d'un `cooperative_reschedule()` toutes les 128 itérations de spin.

### E. ExoFS Open Flags & POSIX Validation (`handlers/fd.rs`, `handlers/fs_posix.rs`, `object_fd/flags.rs`)
- **Rejet des drapeaux modernes (`O_LARGEFILE`, `O_DIRECT`, `O_PATH`, `O_TMPFILE`) :** Les masques `OPEN_FLAGS_MASK` (0x005F_1FFF) dans `handlers/fd.rs` et `handlers/fs_posix.rs` et `VALID_FLAGS` (0x07FF) dans `object_fd/flags.rs` rejetaient les drapeaux standards Linux (`O_LARGEFILE = 0x8000`, `O_DIRECT = 0x4000`, `O_PATH = 0x200000`, `O_TMPFILE = 0x410000`), provoquant `EINVAL` ou `InvalidArgument` sur les appels `open`/`openat`.
- **Correction :** Élargissement du masque à `0x007F_FFFF` de manière cohérente à travers toute la chaîne système et noyau.

### F. Syscall FS Bridge (`file_ops.rs`, `types.rs`)
- **Non-conformité POSIX sur `read`/`write` de répertoires :** `fs_read`, `fs_write`, `fs_pread64`, `fs_pwrite64` tentaient de lire ou d'écrire directement les octets bruts du blob répertoire au lieu de retourner `EISDIR`.
- **Correction :** Vérification systématique de `blob_is_directory_by_id(&entry.blob_id)` et renvoi immédiat de `Err(FsBridgeError::IsDir)`.
- **Non-conformité POSIX sur `lseek` et `ftruncate` de pseudo-blobs :** `lseek` sur un pipe, socket ou eventfd tentait de repositionner un curseur inexistant au lieu de retourner `ESPIPE`. `ftruncate` pouvait tronquer et corrompre les buffers internes de pseudo-blobs.
- **Correction :** Ajout du code d'erreur `IllegalSeek` (`ESPIPE = -29`) dans `FsBridgeError`. Vérification des pseudo-blobs dans `fs_lseek` (retourne `IllegalSeek`) et dans `fs_ftruncate` (retourne `Invalid`).

### G. ExoFS Export & Structures Disque (`export/mod.rs`, `config.rs`, `path_index.rs`, `numa_placement.rs`)
- **Double comptage exponentiel dans `export_snapshot` :** La boucle d'export incrémentait `self.summary.total_blobs_exported`, puis appelait `self.summary.record_export(self.summary.total_blobs_exported, ...)` qui rajoutait à nouveau ce cumulatif à lui-même.
- **Correction :** Comptage local des blobs exportés lors de la session et enregistrement unique via `record_export`.
- **Fuite de mémoire résiduelle dans `object_kind/config.rs` :** La mise à jour d'une valeur plus courte que l'ancienne ne remettait pas à zéro les octets résiduels, polluant le checksum on-disk.
- **Correction :** Remplissage à zéro des octets résiduels `entry.value[value.len()..].fill(0)`.
- **Désérialisation de nom de composant corrompu dans `path_index.rs` :** `from_disk` ne validait pas `d.name_len`, autorisant un `name_len == 0` ou dépassant `PATH_NAME_STORE_LEN`.
- **Correction :** Validation stricte `d.name_len > 0 && d.name_len <= PATH_NAME_STORE_LEN` à la désérialisation et dans `validate()`.
- **Lookup NUMA dans `numa_placement.rs` :** Remplacement de l'heuristique `cpu % n_nodes` par la consultation de `AFFINITY_MAP.node_of_cpu(CpuId(cpu as u32))`.

---

## 2. Validation & Résultats des Tests

- `cargo check --manifest-path kernel/Cargo.toml` : **OK (0 erreur, dev unoptimized)**
- `cargo check --manifest-path drivers/Cargo.toml` : **OK (0 erreur)**
- `make test-drivers` : **OK (22 tests passés, 0 échec)**
- `make test-userspace` : **OK (5 tests passés, 0 échec)**
- `make test-exofs` : **OK (2943 tests passés, 0 échec)**
