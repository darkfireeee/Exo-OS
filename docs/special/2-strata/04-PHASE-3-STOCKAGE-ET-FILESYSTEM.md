# 04 — PHASE 3 : Stockage & Filesystem (ExoFS, VFS, FAT32, EXT4)

## 1. État Réel dans le Dépôt
* **ExoFS & `fs_bridge` (Ring 0)** : Le système de fichiers natif ExoFS est implémenté dans `kernel/src/fs/exofs` et exposé à la fois via les syscalls natifs `500..=520` (`SYS_EXOFS_PATH_RESOLVE` à `SYS_EXOFS_READDIR`) et via le pont POSIX `kernel/src/syscall/fs_bridge.rs` (qui implémente déjà `SYS_GETDENTS64 = 217` et `SYS_GETCWD = 79`)[cite: 7, 18].
* **`vfs_server` (PID 3, Ring 1)** : `servers/vfs_server/src/main.rs` gère la table de montages (`MOUNTS` protégée par `SpinMutex`, max 32 entrées), monte `/proc`, `/sys`, `/dev` par défaut, et sert les opérations `VFS_MOUNT (0)` à `VFS_FSYNC (14)`[cite: 18].
* **Pilotes `fat32` et `ext4` (`drivers/fs/`)** : La crate `exo-os-driver-fs` (`drivers/fs/src/lib.rs`) implémente déjà le parsing BPB FAT32 (`bpb.rs`), les entrées 8.3 et LFN en UTF-16 inversé (`dir_entry.rs`), la table FAT (`fat_table.rs`), ainsi que les inodes, extents et la vérification du journal JBD2 pour ext4 (`ext4/journal.rs`)[cite: 17].
* **Écart restant** : `fat32` et `ext4` sont uniquement des librairies `rlib` dans `drivers/fs/`[cite: 17]. Les démons Ring 1 `fat_server` (PID 12, Vague 3) et `ext_server` exigés par `BOOT_SEQUENCE_STRATA.md` et `ROADMAP-STRATA.md` (§3.3 et §3.4) n'existent pas encore dans `servers/`[cite: 8, 10, 11]. De plus, l'enum `FsType` dans `servers/vfs_server/src/main.rs` ne connaît que `ExoFs (1)`, `ProcFs (2)`, `SysFs (3)` et `DevFs (4)`[cite: 18].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Extension de `servers/vfs_server` pour les Montages FAT32 et EXT4
1. Dans `servers/vfs_server/src/compat/mod.rs` et `src/main.rs`, ajouter :
   * `pub const FS_FAT32: u8 = 5;` et `pub const FS_EXT4: u8 = 6;` dans `FsType`[cite: 18].
   * Un champ `backend_endpoint: u64` dans `MountEntry` permettant à `vfs_server` de déléguer les opérations sur un préfixe monté (comme `/mnt/usb` ou `/mnt/ext4`) au démon `fat_server` ou `ext_server` correspondant[cite: 11, 18].

### B. Création de `servers/fat_server` (PID 12) et `servers/ext_server`
1. Créer `servers/fat_server/Cargo.toml` et `src/main.rs` dépendant de `exo-os-driver-fs = { path = "../../drivers/fs" }`, `exo-storage-common-hal = { path = "../../drivers/storage/common_hal" }` et `exo-syscall-abi = { path = "../syscall_abi" }`[cite: 17, 18].
2. Dans `servers/fat_server/src/main.rs` :
   * S'enregistrer sur l'endpoint `FAT_SERVER_ENDPOINT` (`b"fat_server"`) via `SYS_IPC_REGISTER` (304)[cite: 18].
   * Lors d'une requête de montage sur un périphérique bloc (`virtio_blk_driver` endpoint 32, `ahci_driver` endpoint 30, `nvme_driver` endpoint 31 ou `usb_server`), lire le secteur LBA 0 de la partition via `BlockCmdWire` (`BLOCK_OP_READ = 0x3001`), parser le BPB avec `exo_os_driver_fs::fat32::parse_bpb()`, et vérifier le montage avec `fat32_verify(&bpb, &mut Fat32MountOptions::default_secure())`[cite: 17].
   * Servir les opérations de lecture/écriture de fichiers et de répertoires (`parse_dir_cluster`) en respectant la double écriture `FAT1 + FAT2` (`FS-FAT32-05` dans `write_entry_to_buf`)[cite: 17].
3. Créer `servers/ext_server` sur le même modèle en appelant `exo_os_driver_fs::ext4::compat::verify_before_mount()` et `find_extent()`[cite: 17].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Règles de sécurité `FS-FAT32-01..05` et `FS-EXT4-01..04` (`drivers/fs/src/`)** :
  * **`FS-FAT32-03` (`noexec` immuable)** : `Fat32MountOptions.noexec` doit **toujours** valoir `true`[cite: 17]. Le noyau et `vfs_server` ne doivent jamais autoriser `do_execve()` directement depuis une partition FAT32 ; un binaire provenant d'une clé USB doit d'abord être copié et scanné dans ExoFS[cite: 11, 17].
  * **`FS-EXT4-03` (Journal JBD2 sale)** : Si `ext4/journal.rs` détecte `JournalState::NeedsRecovery`, `verify_before_mount()` retourne `Ext4MountMode::ReadOnly`[cite: 17]. Ne **jamais** tenter de rejouer le journal JBD2 Linux depuis Exo-OS afin d'éviter toute corruption de partition hôte[cite: 17].
* **Garde d'accès `check_vfs_write_access` (`servers/vfs_server/src/main.rs`)** : Actuellement, seuls les PIDs `1`, `3` et `>= 10` ont le droit d'écrire via `vfs_server`, et seul `sender_pid == 1` (`init_server`) est autorisé à appeler `handle_mount()` et `handle_umount()`[cite: 18]. Pour que `device_server` ou `exosh` (`exo mount` / `exo umount`) puissent monter `/mnt/usb`, élargir proprement le contrôle dans `handle_mount` et `handle_umount` aux processus détenant la capability d'administration VFS (ou à `device_server` et `exosh`) sans ouvrir l'écriture aux démons Ring 1 restreints (PIDs 2..9)[cite: 11, 18].