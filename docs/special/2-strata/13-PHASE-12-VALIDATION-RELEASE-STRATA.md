# PLAN-CONVERGENCE-STRATA — Plan d'Exécution vers ExoOS v0.2.0 « Strata »

**Base auditée** : `kernel/`, `exo-boot/`, `drivers/`, `servers/`, `libs/`, `loader/`, `userspace/`
**Cible** : Validation à 100 % de `MASTER-CHECKLIST-STRATA.md`

---

## Chantier 1 — Finalisation de `exo-boot` (BootInfo v2 & NVRAM UEFI — Phase 8)

### Constat dans le code actuel
- `exo-boot/src/disk/gpt.rs` scanne déjà le disque via `exo-partition` (`scan_boot_disk`) et extrait `esp_lba`, `root_lba`, `root_sectors`, `data_lba` et `data_sectors`.
- Toutefois, dans `exo-boot/src/main.rs` (Étape 3b), ce `DiscoveredLayout` est uniquement affiché avec `boot_println!` puis jeté : `handoff.rs` est resté en `BOOT_INFO_VERSION = 1`.
- Le module `exo-boot/src/uefi/nvram.rs` est manquant.

### Actions à réaliser
1. **Étendre `BootInfo` en `BootInfoV2` (`exo-boot/src/kernel_loader/handoff.rs` & `kernel/src/arch/x86_64/boot/`)** :
   - Passer `BOOT_INFO_VERSION` à `2`.
   - Ajouter les champs `exofs_root_phys`, `exofs_root_lba`, `exofs_root_sectors`, `exofs_data_phys`, `exofs_data_lba`, `exofs_data_sectors`, `disk_guid`, `boot_partition_guid`, `boot_from_usb`, `nvme_controller_phys` et `ahci_controller_phys`.
   - Dans `exo-boot/src/main.rs`, injecter le `DiscoveredLayout` retourné par `disk::gpt::scan_boot_disk()` dans `boot_info_ref`.
2. **Implémenter `exo-boot/src/uefi/nvram.rs`** :
   - Créer `register_boot_entry()` utilisant `RuntimeServices::get_variable` / `set_variable` sur `VariableVendor::GLOBAL_VARIABLE` pour enregistrer `BootXXXX` (« ExoOS v0.2.0 — Strata », chemin `\EFI\EXOOS\BOOTX64.EFI`) et mettre à jour `BootOrder` si Secure Boot n'est pas en mode bloquant sans clé enrollée.
3. **Retirer `multiboot2_compat` par défaut (`kernel/Cargo.toml`)** :
   - Basculer le build de production sur le point d'entrée 64-bit `_start_uefi` déjà supporté par `exo-boot`.

---

## Chantier 2 — Déblocage du Pont Réseau, `NetGrant` V5, DNS et TLS 1.3 (Phases 2 & 4)

### Constat dans le code actuel
- `drivers/network/common/src/netgrant.rs` et `servers/syscall_abi/src/netgrant_abi.rs` définissent déjà les structures zéro-copie `NetGrantDesc`, `NetGrantSlot` (2048 octets × 256 slots) et `GrantRing`.
- Mais `kernel/src/syscall/net_bridge.rs` et `servers/network_server/src/protocol.rs` utilisent encore `NET_INLINE_DATA_MAX = 128` octets dans l'enveloppe IPC de 200 octets, rejetant tout paquet > 128 octets avec `EMSGSIZE`.
- `kernel/src/syscall/table/fs/io.rs` (`sys_read` et `sys_write`) ne redirige pas les descripteurs de sockets (`0x4000_0000`) vers `net_bridge`.

### Actions à réaliser
1. **Support des sockets dans `sys_read` et `sys_write` (`kernel/src/syscall/table/fs/io.rs`)** :
   - Ajouter la détection `net_bridge::socket_handle_from_raw(fd)` en entrée de `sys_read`, `sys_write`, `sys_readv` et `sys_writev` (comme c'est déjà fait dans `sys_close`), et déléguer à `net_recvfrom` / `net_sendto`.
2. **Levée de la limite de 128 octets dans `net_bridge.rs` & `network_server`** :
   - Pour les charges utiles > `NET_INLINE_DATA_MAX`, découper les flux TCP en segments dans `net_bridge.rs` ou copier directement depuis/vers l'espace d'adressage du client via `SYS_EXO_MEM_COPY_FROM_PID` (307) / `SYS_EXO_MEM_COPY_TO_PID` (308) dans `network_server` (exactement comme le fait déjà `vfs_server/src/main.rs` dans `handle_read` et `handle_write` avec `CROSS_PROCESS_CHUNK = 4096`).
   - Augmenter `TCP_BUFFER_SIZE` (actuellement 2048 octets dans `servers/network_server/src/smoltcp_iface.rs`) à 16 KiB ou 64 KiB par socket pour supporter les handshakes TLS.
3. **DNS & TLS 1.3 (`servers/network_server` & `libs/exo-network`)** :
   - Intégrer le résolveur DNS (`A`, `AAAA`, `CNAME`) utilisant le serveur DNS obtenu par `dhcp.rs` (`parsed.dns`).
   - Connecter `rustls` (TLS 1.3 strict) aux certificats racines et à `crypto_server`.

---

## Chantier 3 — Conformité `exo-alloc` et Crates Ring 3 (`SPEC-EXO-CRATES-STRATA.md` — Phase 0)

### Constat dans le code actuel
- `libs/exo_allocator` compile actuellement `vendor/mimalloc/src/static.c` en C via `cc::Build`, ce qui contrevient au critère `0.4` / `exo_alloc_test::no_libc_symbol` de `SPEC-EXO-CRATES-STRATA.md`.
- `libs/Cargo.toml` déclare déjà `exo-alloc`, `exo-fs`, `exo-network`, `exo-crypto`, `exo-runtime` et `exo-pkg`.

### Actions à réaliser
1. **Finaliser `libs/exo-alloc` en Rust `#![no_std]` pur** :
   - Implémenter `GlobalAlloc` directement au-dessus de `SYS_MMAP` (9), `SYS_MUNMAP` (11), `SYS_MREMAP` (25) et `SYS_MPROTECT` (10) sans aucun code C ni symbole `malloc`/`free`.
   - Vérifier avec `nm` que `exo-alloc` ne contient aucun symbole libc.
2. **Implémenter `PhoenixSafe` sur les 5 crates natives** :
   - Vérifier que `TcpStream` (`exo-network`), `ExoCryptoContext` (`exo-crypto`), `ExoFile` (`exo-fs`) et `ExoExecutor` (`exo-runtime`) implémentent `on_pre_switch()` et `on_post_switch()`.
3. **Remplacer les stubs de `crypto_server`** :
   - Dans `servers/crypto_server`, remplacer `derive_key_stub` et les retours `CRYPTO_ERR_ARGS` sur `CRYPTO_ENCRYPT`/`CRYPTO_DECRYPT` par les primitives de `drivers/storage/fscrypt` (`exo-fscrypt`) et `ed25519-dalek` / `sha2` / `blake3`.

---

## Chantier 4 — Protocole d'Affichage Natif & Commandes `exo` (`SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md` — Phase 10)

### Constat dans le code actuel
- `servers/syscall_abi/src/display_abi.rs` est déjà conforme pour la partie graphique (`PixelFormat`, `DamageRect`, `SurfaceGrant`).
- En revanche, `servers/exosh/src/builtins/fs.rs` et `util.rs` affichent encore le format POSIX `drwxr-xr-x` avec `uid/gid`, ce qui est formellement interdit par `SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md`.

### Actions à réaliser
1. **Convertir `exosh` au format d'affichage Capability natif** :
   - Utiliser `SYS_EXOFS_PATH_RESOLVE` (500), `SYS_EXOFS_OBJECT_STAT` (506), `SYS_EXOFS_GET_CONTENT_HASH` (508) et `SYS_EXOFS_READDIR` (520).
   - Formater `exo ls -l` selon :
     `<type:d|b|r|s|x|c>  <rights:rwxlksd>  <flags:[sealed]|[✓sig]|[enc]|[usb]|·>  @<token:04x>  ep:<epoch:04>  <size>  <hash:8>  <name>`
   - Formater `exo ls` (court) selon : `<name>  [<type> <rights> <flags>]  <size>`.
2. **Ajouter la commande `exo` dans `exosh`** :
   - Sous-commandes : `exo ls`, `exo stat`, `exo cp`, `exo hash`, `exo mount`, `exo umount`, `exo cap list --pid <pid>`, `exo audit --verify-chain`, `exo log --filter <tag>`, `exo metrics`, `exo install`, `exo compat install`, `exo compat run`, `exo remove`, `exo doctor`.

---

## Chantier 5 — Sous-Système Audio Système (`SPEC-AUDIO-STRATA.md` — Phases 5.4 & 9.2)

### Constat dans le code actuel
- Le trait `AudioDevice` existe dans `drivers/src/lib.rs`, mais il n'existe ni pilote matériel (`hda`, `virtio_sound`), ni serveur `audio_server`, ni gestion du caractère `BEL` (`0x07`) dans `drivers/tty/src/line_disc.rs` ou `servers/tty_server/src/main.rs`.

### Actions à réaliser
1. **Créer `drivers/audio/virtio_sound` et `drivers/audio/hda`** :
   - `virtio_sound` : Device ID 25, 4 virtqueues (`control`, `event`, `tx`, `rx`), flux PCM 44100 Hz, S16LE, stéréo.
   - `hda` : PCI `0x04:0x03`, CORB/RIRB, énumération codec/DAC/Pin, BDL DMA et flux de sortie SDn.
2. **Créer `servers/audio_server` (PID 16, Vague 4)** :
   - Exposer les opcodes IPC `PlaySystemSound (0)`, `Beep (1)` et `Stop (2)`.
   - Embarquer le PCM du chime de démarrage (`G4 392 Hz` 200 ms + `C5 523 Hz` 300 ms) et de l'alerte sécurité (`200 Hz` saturé 1000 ms), ainsi que la synthèse `synthesize_and_play(freq_hz, dur_ms)` avec enveloppe *fade-in* 5 ms / *fade-out* 20 ms.
   - Ajouter `ServiceId::Audio` et les arêtes autorisées (`Init -> Audio`, `Tty -> Audio`, `ExoShield -> Audio`) dans `servers/ipc_router/src/exocordon.rs` et `kernel/src/security/ipc_policy.rs`.
3. **Câbler les 3 émetteurs avec fallback silencieux** :
   - `init_server` : émettre `PlaySystemSound(BootComplete)` après le démarrage d'`exosh` (Vague 6).
   - `tty_server` (`servers/tty_server/src/main.rs`) : sur réception ou écriture de `0x07` (`BEL`), envoyer `Beep(800, 100)` à `audio_server`.
   - `exo_shield` : émettre 3 bips (`300 Hz`, `150 ms`) sur menace `HIGH` et `PlaySystemSound(SecurityAlert)` sur menace `CRITICAL`.

---

## Chantier 6 — Pile USB (xHCI / HID / Mass Storage) & Démons FS Tiers (Phases 3.3, 5.3 & 9.1)

### Constat dans le code actuel
- Les pilotes `fat32` et `ext4` sont déjà écrits dans `drivers/fs/src/fat32/` et `drivers/fs/src/ext4/`, avec application stricte de `noexec` (`FS-FAT32-03`) et vérification du journal JBD2 (`FS-EXT4-03`).
- Il manque les démons `fat_server` / `ext_server` ainsi que toute la pile USB (`xhci`, `ehci`, `hid`, `mass_storage`).

### Actions à réaliser
1. **Créer `drivers/input/usb_hid` et `servers/usb_server` (PID 10)** :
   - Implémenter la détection PCI xHCI (`0x0C:0x03:0x30`) / EHCI (`0x20`), les anneaux de transfert/commande/événement et l'énumération USB.
   - Implémenter le sous-pilote HID Boot Protocol (clavier/souris) poussant des `InputRequest` (`INPUT_MSG_PUSH`) vers `input_server` (exactement comme `drivers/input/ps2/src/main.rs`).
   - Implémenter le sous-pilote Mass Storage Bulk-Only Transport (`bbb.rs` : `CBW`/`CSW` + `scsi.rs` : `INQUIRY`, `READ_CAPACITY`, `READ_10`, `WRITE_10`) exposant l'interface bloc `exo-storage-common-hal`.
2. **Créer `servers/fat_server` (PID 12) et `servers/ext_server`** :
   - Encapsuler `exo-os-driver-fs` (`drivers/fs`) dans deux démons Ring 1 enregistrant leur point de montage auprès de `vfs_server` (`/mnt/usb`).
3. **Câbler le pipeline de transfert USB audité (Phase 9.1)** :
   - Sur attachement d'une clé USB : montage automatique sur `/mnt/usb`, déclenchement d'un scan `exo_shield`, marquage `[usb][scanned ✓]`, vérification du hash BLAKE3 lors de `exo cp /mnt/usb/...` et journalisation `USB_TRANSFER` dans `ExoLedger`.

---

## Chantier 7 — `exo-pkg`, `monitor_server` et Validation Finale (Phases 7, 11 & 12)

1. **Compléter `libs/exo-pkg` et son binaire CLI** :
   - Extraction des archives `.exoar`, vérification de signature Ed25519 via `crypto_server`, affichage interactif du manifeste de capabilities, et génération du profil de sandbox `exo_shield` pour `exo compat install`.
   - Valider les deux jalons obligatoires : `exo compat install calendar` (+ exécution) et `exo compat install curl` (+ `curl https://example.com`).
2. **Créer `servers/monitor_server` (Phase 11)** :
   - Agréger les logs IPC des serveurs Ring 1 et les compteurs `SYS_EXO_PERF_READ` (330), persister les journaux dans ExoFS et répondre aux commandes `exo log` et `exo metrics`.
3. **Exécuter la campagne de validation Phase 12** :
   - Valider les 13/13 tests `security_test`, les 1000 bascules `phoenix_stress_test` (< 500 ms), `cargo deny check`, `semgrep` et `audit_constants.py` avant de tagger `v0.2.0-strata`.