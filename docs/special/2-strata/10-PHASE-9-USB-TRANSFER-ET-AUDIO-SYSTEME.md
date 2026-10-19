# 10 — PHASE 9 : Pipeline USB Transfer & Audio Système

## 1. État Réel dans le Dépôt
* Côté stockage et fichiers, la bibliothèque FAT32 (`drivers/fs/src/fat32`), le serveur `vfs_server` (`servers/vfs_server`) et les syscalls ExoFS (`SYS_EXOFS_GET_CONTENT_HASH = 508`) sont prêts[cite: 7, 17, 18].
* Côté TTY, `drivers/tty/src/line_disc.rs` et `servers/tty_server/src/main.rs` traitent les octets clavier et l'écriture console (`TTY_MSG_WRITE`), mais ignorent encore le caractère `BEL` (`0x07`)[cite: 17, 18].
* **Écarts restants** :
  1. Le serveur `servers/audio_server/` (PID 16, Vague 4) et ses sons embarqués (Boot Chime + Security Alert) + synthèse Beep n'existent pas encore (`9.11`–`9.13`)[cite: 8, 10, 11, 12, 18].
  2. Le serveur `servers/usb_server/` (PID 10, Vague 2) et l'orchestration automatique `USB attach -> mount /mnt/usb -> scan exo_shield -> exo cp + hash check + ExoLedger` n'existent pas encore (`9.1`–`9.10`)[cite: 8, 10, 11, 18].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Création de `servers/audio_server` (PID 16) et Câblage du Son Système (`9.11`–`9.13`)
1. Dans `servers/syscall_abi/src/lib.rs`, définir `pub const AUDIO_SERVER_ENDPOINT: u64 = 16;` ainsi que les constantes `AUDIO_MSG_PLAY_SYSTEM_SOUND: u32 = 0`, `AUDIO_MSG_BEEP: u32 = 1`, `AUDIO_MSG_STOP: u32 = 2`, `SOUND_BOOT_COMPLETE: u32 = 0` et `SOUND_SECURITY_ALERT: u32 = 1` conformément à `SPEC-AUDIO-STRATA.md` §3[cite: 12, 18].
2. Dans `kernel/src/syscall/table/ipc/endpoints.rs` (`service_class_for_endpoint_name`), `kernel/src/security/ipc_policy.rs` et `servers/ipc_router/src/exocordon.rs`, ajouter la classe de service `AudioServer` (`b"audio_server"`) et autoriser uniquement les arêtes :
   * `Init -> Audio` (boot chime)[cite: 12, 18]
   * `Tty -> Audio` (terminal bell `BEL`)[cite: 12, 18]
   * `ExoShield -> Audio` (alertes `HIGH` et `CRITICAL`)[cite: 12, 18]
   *(Conformément à `SPEC-AUDIO-STRATA.md` §3 : aucun processus Ring 3 n'a accès à `audio_server` en v0.2.0)*[cite: 12].
3. Créer `servers/audio_server/` (`#![no_std]`, `#![no_main]`) :
   * Sonder `virtio_sound` en priorité sous QEMU puis `hda` sur bare metal ; si aucun matériel n'est détecté, rester en écoute IPC passive et acquitter les requêtes en silence (`silent_fallback_on_no_hardware`, `9.13`)[cite: 8, 10, 12, 14].
   * Générer les échantillons PCM 44100 Hz S16LE stéréo en arithmétique entière / table sinus précalculée (pour respecter `SRV-02` sans flottants au runtime) :
     * `BootComplete` : note G4 (`392 Hz`, 200 ms) + silence (50 ms) + note C5 (`523 Hz`, 300 ms) avec fade-out[cite: 12].
     * `SecurityAlert` : onde `200 Hz` saturée pendant 1000 ms[cite: 12].
     * `Beep(freq_hz, duration_ms)` : synthèse avec enveloppe fade-in 5 ms et fade-out 20 ms anti-clic[cite: 12].
4. Dans `servers/tty_server/src/main.rs` (`handle_input` et `TTY_MSG_WRITE`), détecter l'octet `0x07` (`BEL`) et envoyer un message IPC non-bloquant `AUDIO_MSG_BEEP` (`freq_hz = 800`, `duration_ms = 100`) vers `AUDIO_SERVER_ENDPOINT` (`9.12`)[cite: 8, 10, 12, 18].
5. Dans `servers/init_server`, dès réception de `RING1_COMPLETE` depuis `exosh` (Vague 6), envoyer `AUDIO_MSG_PLAY_SYSTEM_SOUND(SOUND_BOOT_COMPLETE)` vers `AUDIO_SERVER_ENDPOINT` (`9.11`)[cite: 8, 10, 12].

### B. Pipeline USB $\to$ `/mnt/usb` $\to$ `ExoFS` Audité (`9.1`–`9.10`)
1. Créer `servers/usb_server` (PID 10) pilotant `drivers/input/usb_hid` :
   * Lorsqu'un périphérique Mass Storage BBB est attaché, publier `DEVICE_ATTACHED` vers `device_server`, qui demande à `vfs_server` de monter la partition sur `/mnt/usb` (via `fat_server` si BPB FAT32 valide, ou ExoFS natif, `9.1`, `9.2`)[cite: 8, 10, 11, 14, 17].
   * Déclencher immédiatement une requête IPC `SCAN_REQUEST` vers `exo_shield` sur `/mnt/usb` (`9.3`)[cite: 10, 11].
2. Dans `exo cp <src> <dst>` (lorsque `<src>` commence par `/mnt/usb/`) :
   * Refuser le transfert si l'objet est marqué `[scanned ⚠]` par `exo_shield` (`9.8`)[cite: 10, 11, 16].
   * Calculer le hash BLAKE3 à la volée pendant la lecture de `<src>`, écrire l'objet dans ExoFS, relire le hash de destination via `SYS_EXOFS_GET_CONTENT_HASH` (508) et vérifier `hash_src == hash_dst` (`9.6`)[cite: 7, 10, 11, 18].
   * Enregistrer l'événement `USB_TRANSFER` dans `ExoLedger` (`9.7`)[cite: 10, 11].
3. Sur `exo umount /mnt/usb` (`9.9`), flusher les caches et envoyer `VFS_UMOUNT` (1) à `vfs_server` ; sur retrait physique sans `umount` (`MassStorageDetached`), journaliser `USB_UNEXPECTED_REMOVAL` (`9.10`)[cite: 10, 11, 14, 18].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Mise à jour synchronisée du DAG `ExoCordon` (Règle `IPC-01`)** : Dans `servers/ipc_router/src/exocordon.rs`, une assertion compile-time vérifie `AUTHORIZED_GRAPH.len() == 51` en miroir strict de `kernel/src/security/ipc_policy.rs::POLICY`[cite: 18]. Si vous ajoutez `ServiceId::Audio` ou `ServiceId::Usb` et leurs arêtes IPC, vous **devez obligatoirement mettre à jour simultanément** :
  1. `kernel/src/security/ipc_policy.rs` (`ServiceClass` + `POLICY`)[cite: 7, 18]
  2. `kernel/src/syscall/table/ipc/endpoints.rs` (`service_class_for_endpoint_name`)[cite: 7]
  3. `servers/ipc_router/src/exocordon.rs` (`ServiceId`, `service_id_for_name`, `AUTHORIZED_GRAPH` et la constante de longueur)[cite: 18].
* **Fallback Audio non-bloquant** : Un échec ou timeout d'envoi IPC vers `audio_server` ne doit **jamais** faire paniquer `init_server`, `tty_server` ou `exo_shield`, ni bloquer la boucle d'événements clavier du TTY (`timeout <= 50 ms`)[cite: 8, 12, 18].