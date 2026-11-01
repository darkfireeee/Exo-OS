# Audit Approfondi #54 : Intégrité Mémoire object_stat, Validation path_ops, Atomicité IPC & Streaming Dirent Rust std

**Date :** 1er Novembre 2026  
**Auteur :** Antigravity (Advanced Agentic Coding)  
**Domaines audités :**
1. **FS / ExoFS Object Stat (SYS 506) :** Élimination du débordement de pile (72 octets) via support du layout compact 104 octets `ExoFsFileStat` pour Rust native `library/std`.
2. **FS / ExoFS Open By Path (SYS 519) :** Élargissement du masque `VALID_FLAGS` (0x007F_FFFF) admettant `O_PATH` et `O_TMPFILE`, et vérification de collision atomique `O_CREAT | O_EXCL` (`-EEXIST`).
3. **SYSCALL / POSIX path_ops :** Conformité POSIX man 2 `*at` pour les chemins absolus débutant par `/` (`mkdirat`, `unlinkat`, `fchmodat`, `fchownat`).
4. **IPC Endpoint Lifecycle :** Élimination de la faille TOCTOU dans `endpoint_destroy` via atomisation sous verrou unique de la vérification `active_conns`, de l'invalidation du slot et de la libération du bitmap.
5. **IPC Shared Memory Mapping :** Prévention de la fuite de compteur de références (`active_mappings`) dans `shm_map` par validation des permissions d'écriture en amont de `add_mapping`.
6. **Rust Native std Directory Traversal :** Implémentation du streaming continu de répertoires dans `std::fs::read_dir` (`ReadDir`) sur flux `LinuxDirent64` unifié via `SYS_GETDENTS` (78 / 217) et `SYS_EXOFS_READDIR` (520).

---

## 1. Contexte & Enjeu Rust `std`

Dans l'architecture micro-noyau moderne d'Exo-OS, l'espace utilisateur (shell `exosh`, utilitaires `coreutils`, serveurs Ring 1) repose désormais sur la véritable bibliothèque standard Rust native (`target_os = "exo"`). Les structures binaires échangées via les appels système doivent garantir une compatibilité ABI stricte avec la libc/std sans compromettre la sécurité du noyau ni corrompre la pile utilisateur.

---

## 2. Vulnérabilités & Incohérences Détectées et Corrigées

### 2.1 FS / ExoFS : Débordement de pile dans `sys_exofs_object_stat` (SYS 506)
- **Fichier :** `kernel/src/fs/exofs/syscall/object_stat.rs`.
- **Problème :** La bibliothèque standard Rust (`library/std/src/sys/pal/exo_os/syscall.rs:592`) et `libs/exo-syscall` appellent `sys_exofs_object_stat(fd, &mut st, 104)`. La structure attendue en mémoire utilisateur est `ExoFsFileStat` de **104 octets**. Le noyau, quant à lui, attendait 6 arguments avec le pointeur en 4ème position (`out_ptr`) et écrivait inconditionnellement la structure noyau interne `ObjectStat` de **176 octets**.
  - **Conséquence critique :** Écriture de 176 octets dans un buffer de 104 octets sur la pile utilisateur, écrasant 72 octets de frame pointer et d'adresses de retour, menant à des crashes sporadiques ou des corruptions d'état lors de `std::fs::metadata`.
- **Correction :**
  - Définition formelle de `ExoFsFileStat` (104 octets, vérifiée par assert statique de compilation).
  - Détection automatique de la convention d'appel à 3 arguments (`out_ptr == 0 && _length != 0 && flags == 104`).
  - Conversion sécurisée de l'état interne vers `ExoFsFileStat` et copie bornée aux 104 octets attendus si le caller utilise ce format, tout en préservant le format étendu `ObjectStat` (176 octets) pour les outils système internes.

### 2.2 FS / ExoFS : Masque d'ouverture et sémantique `O_EXCL` dans `open_by_path` (SYS 519)
- **Fichier :** `kernel/src/fs/exofs/syscall/open_by_path.rs`.
- **Problème 1 :** Le masque `flags & !0x000F_FFFF != 0` rejetait des drapeaux valides comme `O_PATH` (0x0020_0000) et `O_TMPFILE` (0x0040_0000).
- **Problème 2 :** L'association `O_CREAT | O_EXCL` ne retournait pas `-EEXIST` lorsque le fichier existait déjà, écrasant silencieusement les fichiers existants ou ignorant le contrat POSIX attendu par `OpenOptions::create_new(true)`.
- **Correction :**
  - Utilisation du masque complet `crate::fs::exofs::syscall::object_fd::open_flags::VALID_FLAGS` (0x007F_FFFF).
  - Vérification d'existence préalable sous `flags & O_CREAT != 0 && flags & O_EXCL != 0` via `resolve_path_to_blob`. En cas d'existence, retour immédiat de `ExofsError::AlreadyExists` (`-EEXIST`).

### 2.3 SYSCALL / POSIX : Prise en charge des chemins absolus dans `path_ops` (*at)
- **Fichier :** `kernel/src/syscall/table/fs/path_ops.rs`.
- **Problème :** Dans `sys_mkdirat`, `sys_unlinkat`, `sys_fchmodat`, `sys_fchownat`, le code rejetait inconditionnellement tout appel où `dirfd != AT_FDCWD` avec `ENOSYS`. Cependant, la norme POSIX man 2 stipule : *"If pathname is absolute, then dirfd is ignored"*.
- **Correction :** Si `path.as_bytes().starts_with(b"/")`, l'opération ignore `dirfd` et s'exécute directement sur le chemin absolu via `fs_bridge`.

### 2.4 IPC : Élimination de la faille TOCTOU dans `endpoint_destroy`
- **Fichier :** `kernel/src/ipc/endpoint/lifecycle.rs`.
- **Problème :** `endpoint_destroy` lisait le compteur `active_conns` sous un premier verrou, relâchait le verrou, puis réacquérait le verrou pour invalider le slot du pool et libérer le bit dans le bitmap. Un thread concurrent pouvait initier une connexion dans cette fenêtre temporelle, menant à une destruction d'un endpoint actif.
- **Correction :** Réalisation de la vérification `active_conns == 0`, de la réinitialisation du slot (`pool[idx] = None`), de la purge de la queue de messages et de la libération du bitmap (`release_id`) sous une seule et unique acquisition atomique du verrou.

### 2.5 IPC : Prévention de fuite de référence dans `shm_map`
- **Fichier :** `kernel/src/ipc/shared_memory/mapping.rs`.
- **Problème :** Dans `shm_map`, le descripteur voyait son compteur de mappings incrémenté via `desc.add_mapping()` avant que les permissions d'écriture demandées ne soient confrontées aux permissions réelles du bloc (`rp.can_write()`). En cas de permission insuffisante, une erreur était retournée mais le compteur atomique restait gonflé.
- **Correction :** Validation des permissions d'écriture en amont de `desc.add_mapping()`.

### 2.6 Rust Native std : Streaming continu de répertoire `LinuxDirent64`
- **Fichiers :** `library/std/src/sys/fs/exo.rs`, `library/std/src/sys/pal/exo_os/syscall.rs`.
- **Problème :** `std::fs::read_dir` (`readdir`) utilisait un tableau fixe de 64 entrées `ExoDirEntry` (256 octets), alors que le noyau produit un flux variable continu de `LinuxDirent64`.
- **Correction :**
  - Implémentation du streaming dans `readdir` par bouclage de lecture (buffer de 4096 octets) via `SYS_GETDENTS` / `SYS_EXOFS_READDIR`.
  - Décodage à la volée des enregistrements variables `LinuxDirent64` (`d_ino`, `d_reclen`, `d_type`, `name`), permettant de lister des répertoires contenant un nombre arbitraire d'entrées.
  - Maintien d'un convertisseur transparent dans `exofs_readdir` pour la compatibilité avec les structures fixes existantes.

---

## 3. Résultats des Tests & Vérifications

1. **Kernel ExoFS Test Suite :**
   ```
   test result: ok. 2943 passed; 0 failed; 0 ignored; 0 measured; 183 filtered out; finished in 83.02s
   ```
   - 100% de réussite sur les 2943 tests unitaires et d'intégration d'ExoFS.
2. **Pilotes Matériels (`make test-drivers`) :**
   - PS/2 Input : 7 passed, 0 failed.
   - TTY Line Discipline : 7 passed, 0 failed.
   - VGA Text : 2 passed, 0 failed.
   - VirtIO Blk : 6 passed, 0 failed.
3. **Userspace Shell & Coreutils (`make test-userspace`) :**
   - Coreutils lib : 1 passed, 0 failed.
   - Shell exosh parser & exec : 2 passed, 0 failed.
4. **Compilation Rust Native std :**
   - `exo-exosh` compilé avec succès pour `x86_64-unknown-exo.json` avec `library/std`.
   - `coreutils` (30 binaires) compilés avec succès pour `x86_64-unknown-exo.json` avec `library/std`.
