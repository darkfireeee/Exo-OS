# RAPPORT D'AUDIT APPROFONDI (PHASE 2) : IPC, SCHEDULER, SYSCALL ET EXOFS

**Date :** 29 Octobre 2026  
**Cible :** `x86_64-unknown-none` (Micro-Noyau Exo-OS Ring 0)  
**Sous-systèmes audités :** IPC, Ordonnanceur (Scheduler), Appels Système (Syscalls), Système de Fichiers (ExoFS Dedup & Objects)

---

## 1. Synthèse Exécutive

Dans le cadre de la session d'audit exhaustif et approfondi menée sur les dossiers de signalements (`Analyse_2`, `Analyse_z-ai`, `Analyse (3)`), une seconde phase d'investigation minutieuse a été conduite sur les sous-systèmes névralgiques du noyau :
- **6 nouvelles anomalies critiques et majeures** ont été identifiées dans le code source, contre-vérifiées avec précision mathématique et corrigées :
  1. Élimination des fuites de spinlock (deadlocks permanents du noyau) sur échec d'incrémentation ou d'allocation dans les registres dedup (`chunk_index.rs`, `blob_registry.rs`, `blob_sharing.rs`).
  2. Correction du blocage d'insertion dans `ConfigStore::set` où la mise à jour d'une clé existante était rejetée à tort avec `NoSpace` quand la table atteignait sa capacité maximale (`config.rs`).
  3. Élimination de la corruption des entrées d'index de chemins `PathIndexEntry` par rejet immédiat des composants excédant la capacité de stockage fixe de 128 octets (`path_index.rs`), évitant les entrées orphelines et ineffaçables.
  4. Borne stricte POSIX `UIO_MAXIOV` / `IOV_MAX = 1024` sur `sys_preadv` et `sys_pwritev` (`table/fs/io.rs`) pour éliminer le vecteur de déni de service par épuisement de ressources (boucle jusqu'à $2^{32}-1$ iovecs).
- **Vérifications et Réfutations Techniques :**
  - Validation de `kernel/src/ipc/sync/futex.rs` : délègue directement à `memory::utils::futex_table`, où le requeue in-place (`requeue_inplace`) a été validé.
  - Validation du fail-closed de `Credentials::nobody()` dans `fast_path.rs`.
  - Validation de `sys_tgkill` : contrôle d'autorisation UID/EUID effectif et ciblage par `tid` sur les `thread_slots`.
  - Validation des barrières d'epoch NVMe : mode dégradé instrumenté empêchant le blocage du démarrage bare-metal en l'absence de contrôleur physique actif.

---

## 2. Détail des Bogues Corrigés et Justifications Techniques

### 2.1 ExoFS Dedup : Fuites de Spinlock sur Échec (`chunk_index.rs`, `blob_registry.rs`, `blob_sharing.rs`)
- **Problème :** Dans les méthodes `insert`, `register`, `add_ref`, `orphan_keys`, `orphan_blobs` et `all_shared_chunks`, le verrou de table était acquis via `self.acquire()`. Les opérations internes utilisaient l'opérateur `?` (`e.inc_ref()?`, `keys.try_reserve(1)?`, `BlobEntry::new()?`). En cas d'erreur (saturation de compteurs de référence ou échec d'allocation mémoire `try_reserve`), la fonction retournait immédiatement sans exécuter `self.release()`. Le spinlock restait à 1 pour toujours, provoquant un gel irrécupérable du noyau au prochain accès à la table dedup.
- **Correction :** Encadrement systématique de toutes les opérations faillibles sous verrou : capture explicite du résultat avec `match` / `if let Err(err)`, appel garanti à `self.release()` avant propagation de l'erreur.

### 2.2 ExoFS Objects : Mise à Jour sur Table Config Pleine (`config.rs`)
- **Problème :** Dans `ConfigStore::set`, la condition `if self.entries.len() >= CONFIG_MAX_ENTRIES` était évaluée avant la recherche de clé existante. Lorsqu'une table de configuration atteignait sa capacité maximale sans tombstone, toute tentative de mise à jour de la valeur d'une clé existante échouait avec `Err(ExofsError::NoSpace)`, alors qu'une mise à jour ne consomme aucun nouveau slot.
- **Correction :** La recherche d'une clé existante pour mise à jour in-place est désormais exécutée en priorité avant toute vérification de capacité de la table.

### 2.3 ExoFS Objects : Tronquage Silencieux et Entrées Injoignables (`path_index.rs`)
- **Problème :** `PathIndexEntryDisk` stocke un nom de composant de longueur fixe de 128 octets (`PATH_NAME_STORE_LEN = 128`). Le constructeur `PathIndexEntry::new` acceptait des noms jusqu'à `PATH_NAME_MAX = 255`, stockait les 128 premiers octets mais conservait `name_len > 128`. Comme `name_bytes()` retourne une tranche bornée à 128 octets, `name_bytes() == name` dans `lookup()` et `remove()` ne correspondait jamais pour les noms de 129 à 255 octets. Le slot restait occupé indéfiniment sans pouvoir être ni résolu ni supprimé.
- **Correction :** Rejet strict des noms excédant `PATH_NAME_STORE_LEN` avec `Err(ExofsError::InvalidPathComponent)`.

### 2.4 Syscalls FS : Borne IOV_MAX sur `sys_preadv` et `sys_pwritev` (`table/fs/io.rs`)
- **Problème :** Contrairement à `sys_readv` et `sys_writev` qui bornaient `iovcnt` à 1024 (`UIO_MAXIOV`), `sys_preadv` et `sys_pwritev` acceptaient jusqu'à `u32::MAX` iovecs (4,29 milliards). Cela ouvrait une opportunité de déni de service dans la boucle de résolution utilisateur.
- **Correction :** Ajout de la garde explicite `if iovcnt > 1024 { return EINVAL; }` au point d'entrée des syscalls `sys_preadv` et `sys_pwritev`.

---

## 3. Matrice de Validation

| Fichier Modifié | Anomalie | Gravité | Correction Appliquée | Statut |
| :--- | :--- | :---: | :--- | :---: |
| `kernel/src/fs/exofs/dedup/chunk_index.rs` | Fuite de spinlock `inc_ref` & `orphan_keys` | Critique | `self.release()` garanti sur toute sortie d'erreur | Validé |
| `kernel/src/fs/exofs/dedup/blob_registry.rs` | Fuite de spinlock `register` & `orphan_blobs` | Critique | `self.release()` garanti sur toute sortie d'erreur | Validé |
| `kernel/src/fs/exofs/dedup/blob_sharing.rs` | Fuite de spinlock `add_ref` & `all_shared_chunks` | Critique | `self.release()` garanti sur toute sortie d'erreur | Validé |
| `kernel/src/fs/exofs/objects/object_kind/config.rs` | Rejet erroné `NoSpace` sur table pleine | Majeur | Update prioritaire sur la vérification de capacité | Validé |
| `kernel/src/fs/exofs/objects/object_kind/path_index.rs` | Entrées fantômes orphelines >128 B | Majeur | Rejet des noms > 128 B avec `InvalidPathComponent` | Validé |
| `kernel/src/syscall/table/fs/io.rs` | Vecteur DoS `iovcnt` non borné sur preadv/pwritev | Majeur | Borne stricte POSIX `iovcnt > 1024` $\to$ `EINVAL` | Validé |
