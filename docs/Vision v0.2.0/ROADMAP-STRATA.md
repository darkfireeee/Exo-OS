# ROADMAP-STRATA — Plan d'Implémentation ExoOS v0.2.0
## 12 Phases, Dépendances Complètes, Ordre de Développement

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace ROADMAP-IMPLEMENTATION-V0.2.md

---

## Vue d'Ensemble

```
PRÉREQUIS   — BLOC -1 : Bugs kernel bloquants        (avant tout)
PRÉREQUIS   — BLOC  0 : Outillage audit               (avant tout)

PHASE  0 — Fondations Runtime                         (bloquant tout)
PHASE  1 — Chaîne de Sécurité Active                  (bloquant toute app)
PHASE  2 — Services Réseau & Cryptographie            (bloquant apps réseau)
PHASE  3 — Stockage & Filesystem                      (bloquant installeur)
PHASE  4 — Processus & POSIX Compat                   (bloquant exo compat)
PHASE  5 — Drivers Bare Metal                         (bloquant hardware réel)
PHASE  6 — ExoShield Ring1 Intégration Complète       (bloquant sécurité prod)
PHASE  7 — Installeur & PKG                           (bloquant apps POSIX)
PHASE  8 — Bootloader UEFI GPT                        (bloquant bare metal boot)
PHASE  9 — USB Transfer + Audio Système               (fonctionnalité ordi complet)
PHASE 10 — Shell & Framebuffer                        (expérience utilisateur)
PHASE 11 — Observabilité & Qualité                    (stabilité ≥98%)
PHASE 12 — Validation Release Strata                  (release candidate)
```

---

## PRÉREQUIS — BLOC -1 : Bugs Kernel Bloquants

**Seuil :** 10/10 avant de démarrer Phase 0.

| # | Bug | CORR | Critère de validation |
|---|---|---|---|
| B-01 | VirtIO BAR lu depuis PCI config space (plus hardcodé) | CORR-86 | Log BAR0 != 0x10000000 |
| B-02 | ExoFS persiste sur disque après reboot | CORR-86 | `cat /test.txt` après reboot |
| B-03 | Boot avec `-m 2G` sans panic kernel | CORR-76 | Log "physmap étendue : 2 GiB" |
| B-04 | `phys_to_virt()` sur adresse > 1 GiB retourne valide | CORR-76 | Test unitaire |
| B-05 | `cgroup::init()` appelé avant `runqueue_init()` | CORR-77 | Log "root cgroup valide" |
| B-06 | Ring1 servers attachés au root cgroup sans crash | CORR-77 | `exo ps` → tous running |
| B-07 | ELF base 0x400000 accepté par l'ELF loader | CORR-80 | Binary hello charge |
| B-08 | `USER_ELF_BASE_MIN ≤ 0x400000` (const_assert!) | CORR-80 | `cargo build` → pass |
| B-09 | MSG len==128 sans cap → PolicyDenied (pas injection PID) | CORR-78 | Test pénétration |
| B-10 | exosh démarre sans network_server (QEMU -net none) | CORR-79 | Boot sans réseau → shell visible |

---

## PRÉREQUIS — BLOC 0 : Outillage d'Audit

**Seuil :** 13/13 avant de démarrer Phase 0.

| # | Critère |
|---|---|
| O-01 | `arch/constants.rs` créé avec toutes les constantes canoniques |
| O-02 | `const_assert!` dans ssr.rs (SSR size ≤ 4096) |
| O-03 | `const_assert!` dans exokairos.rs (KAIROS_WINDOW_NS) |
| O-04 | `const_assert!` dans physmap.rs (PHYSMAP_INITIAL_COVERAGE) |
| O-05 | `const_assert!` cohérence CORE_MASK_WORDS × 64 == MAX_CORES_LAYOUT |
| O-06 | `tools/audit_constants.py` créé et fonctionnel |
| O-07 | `audit_constants.py` → 0 erreurs sur kernel/ |
| O-08 | `tools/semgrep-rules/exoos.yaml` créé |
| O-09 | Semgrep → 0 violations sur kernel/ |
| O-10 | `deny.toml` configuré (libsodium, dbus, zbus, tokio-runtime interdits) |
| O-11 | `cargo deny check` → 0 violations |
| O-12 | Pre-commit hook installé et fonctionnel |
| O-13 | `.github/workflows/audit.yml` créé |

---

## PHASE 0 — Fondations Runtime

**Objectif :** Sans cette phase, aucune crate Ring3 ne compile.

### 0.1 — Fix SSR Bitmask 256-core [P0 BLOQUANT]

**Fichiers :** `forge.rs`, `handoff.rs`, `isolate.rs`, `ssr.rs`

```rust
// AVANT :  pub active_cores: u64,
// APRÈS :  pub active_cores: [u64; CORE_MASK_WORDS],  // 4 × u64 = 256 cores
```

**Test :** `phoenix_test::ssr_bitmask_256_cores` → PASS

### 0.2 — exo-alloc : Allocateur Userland [P0 BLOQUANT]

**Fichiers :** `libs/exo-alloc/src/`

- `DlmallocAllocator<Exo>` implémente `GlobalAlloc`
- `#[global_allocator]` pointe vers l'allocateur ExoOS
- Zéro import `libc`, `malloc`, `free`, `sbrk`
- `exo_mmap_anon()` → `SYS_MMAP` avec `MAP_ANON | MAP_PRIVATE`
- `exo_mremap()` pour `realloc`

**Tests :**
```
exo_alloc_test::alloc_basic_sizes      PASS
exo_alloc_test::dealloc_correct        PASS
exo_alloc_test::realloc_grow           PASS
exo_alloc_test::alignment_respected    PASS
exo_alloc_test::concurrent_alloc_free  PASS
```

### 0.3 — generic-rt : TLS + TCB Access [P0 BLOQUANT]

- `__tls_get_addr` implémenté pour ELF dynamiques
- `gs:[0x20]` current TCB accessible depuis Ring3
- TLS initialisées avant `main()`
- TLS survit à `clone()` (thread indépendant)

---

## PHASE 1 — Chaîne de Sécurité Active

**Objectif :** Les 8 composants de sécurité actifs en production — pas de stub.

### 1.1 — ExoSeal [P0]

- `exoseal_verify_boot_chain()` en Phase 0 boot (avant tout)
- Hash kernel binaire vérifié
- Hash servers Ring1 vérifiés
- Mode QEMU : hash en mémoire (`EXOSEAL_DEV_BYPASS` → ExoLedger)

### 1.2 — ExoCage : Hardware Mitigations [P0]

- SMEP (CR4.SMEP) : BSP + tous APs
- SMAP (CR4.SMAP) : BSP + tous APs
- KPTI : toutes transitions kernel↔user
- CET Shadow Stack (MSR_IA32_U_CET)
- CET IBT (MSR_IA32_S_CET)
- NX/XD (EFER.NXE)
- IBRS + SSBD (Spectre mitigations)
- `exocage_verify_active()` après Phase 5, panic si incomplet

### 1.3 — Zero Trust sur IPC [P0]

- `ZeroTrustLabel` attaché à chaque message SpscRing
- `zero_trust::check_ipc()` sur chaque `ipc_send` fast path
- Ring3→Ring3 direct IPC bloqué (sauf SHM autorisé)
- Fast path bitmask Ring1↔Ring1 (ERR-09 corrigé)
- IPC non conforme : bloqué + ExoLedger + ExoKairos débit

### 1.4 — CapToken : Couverture Complète [P0]

- Chaque accès FS passe par `capability::verify()`
- Chaque IPC vers Ring1 vérifié
- Chaque accès réseau vérifié
- Révocation propagée immédiatement à tous les tokens dérivés
- Test : cap révoquée → `EXO-0410` instantané

### 1.5 — ExoKairos : Budgets Temporels [P1]

- Budget initialisé à chaque création processus Ring3 (100ms/s par défaut)
- `update_kairos_budget()` à chaque context switch
- Throttle à 100% budget (pas kill)
- Kill à 200% cumulé
- Fenêtre de reset documentée (ERR-07 corrigé)
- ExoLedger entrée sur dépassement

### 1.6 — ExoLedger : Persistance & Chaîne [P0]

- Journal persisté dans ExoFS (objet sealed)
- Chaîne BLAKE3 vérifiée au boot
- `exo audit` fonctionnel avec vérification de chaîne
- Impossible de modifier/supprimer une entrée passée

### 1.7 — ExoShield IOMMU Statique [P0]

*(Distinct du serveur ExoShield Ring1 — c'est le composant kernel)*

- IOMMU activé avant tout driver Ring1
- Domaines séparés : NET / BLOCK / BLACKHOLE
- `SYS_DMA_ALLOC=534` retourne (virt, iova) correct
- `IommuFaultQueue` actif (CAS-strong)
- Test DMA hors plage → fault détectée + DMA stoppé

### 1.8 — ExoNMI : Watchdog [P1]

- NMI watchdog armé toutes les 200ms
- Heartbeat incrémenté → visible par ExoPhoenix sentinel
- Canaries stack kernel vérifiés dans handler NMI
- IDT integrity check au NMI

---

## PHASE 2 — Services Réseau & Cryptographie

### 2.1 — crypto_server Complet [P0]

- TRNG matériel (RDRAND/RDSEED) source primaire
- AES-GCM-256, ChaCha20-Poly1305 (rustcrypto-aeads)
- SHA-256, SHA-3, BLAKE3 (rustcrypto-hashes)
- Argon2id (rustcrypto-password-hashes)
- ECDSA P-256 + Ed25519 (rustcrypto-elliptic-curves)
- RSA-OAEP (rustcrypto-rsa)
- HKDF (rustcrypto-kdfs)
- Clés privées jamais exportées hors serveur
- Tests round-trip + vecteurs NIST

### 2.2 — network_server : smoltcp + dhcp4r + hickory-dns [P0]

- smoltcp IPv4/IPv6 dans boucle principale
- DHCP automatique via dhcp4r
- DNS via hickory-dns (A, AAAA, CNAME)
- `sys_sched_yield()` dans boucle principale (pas busy-loop)
- TCP + UDP fonctionnels
- Zéro `panic!` (tous chemins d'erreur propagés)

### 2.3 — rustls TLS 1.3 [P1]

- TLS 1.3 uniquement (1.2 désactivé)
- Certificats racine depuis keystore crypto_server
- Test : `curl https://example.com` → 200 OK

---

## PHASE 3 — Stockage & Filesystem

### 3.1 — ExoFS 4-phase fsck en Production [P0]

- fsck phase 1 (inode scan) → PASS
- fsck phase 2 (directory check) → PASS
- fsck phase 3 (connectivity check) → PASS
- fsck phase 4 (bad block) → PASS
- Recovery automatique après crash simulé

### 3.2 — vfs_server : Primitives ExoFS Natives [P0]

- `open`, `close`, `read_at`, `write_at`
- `mkdir`, `rmdir`, `unlink`, `rename` (O(1) dans ExoFS)
- `stat` avec champs ExoFS natifs
- `readdir` / `getdents64` (SYS_GETDENTS64 = 217 implémenté)
- `SYS_GETCWD` (79) implémenté
- Epochs : commit atomique après chaque écriture
- Snapshots : `snapshot_create()` opérationnel
- Relations typées : `relation_create()` opérationnel
- Content hash : `get_content_hash()` accessible

### 3.3 — fat_server [P1]

- Montage volumes FAT32 via virtio-blk
- Lecture/écriture fichiers FAT32
- Exposition via vfs_server (mount point dans ExoFS)
- Test : monter image FAT32 QEMU, lire un fichier

### 3.4 — ext_server [P1]

- Accès blocs bruts NVMe/AHCI via ext_server
- Montage ext4 (ext4-rs)
- Test : lire partition ext4 depuis QEMU

---

## PHASE 4 — Processus & POSIX Compat

### 4.1 — Fork/CoW Fixes [P0 BLOQUANT]

- TLB shootdown deadlock → skip si single-CPU
- VMA tree non cloné → deep clone VMA tree
- `KERNEL_FAULT_ALLOC` mauvais espace → vérifier CR3 avant CoW

### 4.2 — musl-exo : 127 Syscalls [P0]

- Priorités 1 et 2 selon `SPEC-EXO-LIBC.md`
- Tests : `fork_exec_wait` PASS, `socket_tcp_connect` PASS, `getdents64` PASS
- `SYS_CLONE` avec tous flags utiles (CLONE_THREAD, CLONE_VM, CLONE_FS)
- `SYS_FUTEX` (FUTEX_WAIT, FUTEX_WAKE)

---

## PHASE 5 — Drivers Bare Metal

**Objectif :** ExoOS fonctionne sur matériel physique, pas seulement en VM.

### 5.1 — AHCI : SATA Bare Metal [P1]

**Fichiers :** `drivers/storage/ahci/src/`

- Détection contrôleur AHCI via PCI (class 0x01, subclass 0x06)
- Init HBA : AHCI enable (GHC.AE), reset
- Détection ports actifs (HBA.PI bitmask)
- Port init : command list + FIS receive area (4K alignés)
- Command slot allocation + command FH build
- READ/WRITE DMA Extended (ATA CMD 0x25/0x35)
- NCQ (Native Command Queuing) si supporté
- Interrupts AHCI → Ring1 IPC
- Test : lire premier secteur d'un disque SATA QEMU

### 5.2 — NVMe : SSD Bare Metal [P1]

**Fichiers :** `drivers/storage/nvme/src/`

- Détection contrôleur NVMe via PCI (class 0x01, subclass 0x08)
- Init contrôleur : CC.EN, CSTS.RDY
- Admin Queue (submission + completion, 64 entries chacune)
- I/O Queue (au moins 1 paire SQ/CQ)
- Identify Controller + Identify Namespace (NSID=1)
- Commandes NVM : Read (opcode 0x02), Write (opcode 0x01)
- Interrupts MSI-X prioritaires, MSI en fallback, polling en dernier recours
- Test : lire premier bloc d'un NVMe QEMU

### 5.3 — USB HID + Mass Storage [P1]

**Fichiers :** `drivers/input/usb_hid/src/`

**USB HID (clavier/souris) :**
- XHCI controller detection (PCI class 0x0C, subclass 0x03, prog-if 0x30)
- EHCI fallback (prog-if 0x20)
- Énumération USB : reset, address assignment, descriptor read
- HID class driver : keyboard (boot protocol) + mouse
- Événements routés vers input_server (même format que PS/2)

**USB Mass Storage (clés USB) :**
- BBB (Bulk-Only Transport) protocol
- SCSI command set : INQUIRY, READ_CAPACITY, READ_10, WRITE_10
- Détection automatique à l'énumération USB
- Événement `DEVICE_ATTACHED` → device_server → vfs_server mount

### 5.4 — Audio : HDA + virtio-sound [P1]

**Fichiers :** `drivers/audio/hda/src/`, `drivers/audio/virtio_sound/src/`

**Périmètre v0.2.0 — Audio Système Uniquement :**

HDA (Intel HD Audio) :
- Détection PCI (class 0x04, subclass 0x03)
- CORB/RIRB initialization
- Codec enumeration (codec address, widget discovery)
- Output DAC path : Line Out ou Headphones
- PCM output : 44100 Hz, 16-bit, stereo (ou mono selon codec)
- Interface IPC vers audio_server

virtio-sound (QEMU/VM) :
- VirtIO device negotiation (VIRTIO_DEVICE_ID = 25)
- PCM stream setup : 44100 Hz, stereo, S16LE
- Virtqueue tx : pcm_xfer + pcm_release cycle
- Interface IPC identique à HDA (même audio_server API)

**audio_server (Ring1, Vague 4) :**
- Démarre après tty_server et input_server
- 3 services uniquement :
  - `PLAY_SYSTEM_SOUND(sound_id)` : chime, bell, alert
  - `BEEP(freq_hz, duration_ms)` : PC speaker style
  - `STOP()` : arrêt immédiat
- Sons embarqués en mémoire (PCM statique dans le binaire)
- Pas de lecture fichiers audio, pas de mixer

### 5.5 — Driver Framework & Manager [P2]

**Fichiers :** `drivers/framework/src/`, `drivers/manager/src/`

- Traits unifiés : `BlockDevice`, `NetDevice`, `InputDevice`, `AudioDevice`
- Hot-plug event bus : DEVICE_ATTACHED / DEVICE_DETACHED
- Dependency graph pour device_server
- Driver probe/bind/unbind lifecycle

### 5.6 — Clock Driver [P1]

**Fichiers :** `drivers/clock/src/`

- RTC (Real Time Clock) : lecture heure/date au boot
- HPET : high precision timer pour timestamps précis
- Synchronisation avec ktime_get_ns() kernel
- Interface IPC vers scheduler_server

---

## PHASE 6 — ExoShield Ring1 : Intégration Complète

**Objectif :** exo_shield démarre en Vague 5 (dernier Ring1) et prend en charge la surveillance complète du système.

### 6.1 — Hooks Kernel Câblés [P0]

- `hooks/syscall_hooks.rs` : callback sur chaque syscall Ring3
- `hooks/exec_hooks.rs` : callback sur `execve`, `execveat`
- `hooks/memory_hooks.rs` : callback sur `mmap`, `mprotect` suspects
- `hooks/net_hooks.rs` : callback sur connect, bind, sendto

Interface kernel → exo_shield : IPC `EVENT_REPORT(1)` avec payload structuré.
Zéro chemin de retour bloquant dans les hooks (async ring buffer).

### 6.2 — Signatures YARA au Boot [P0]

- Base de signatures chargée depuis `/etc/exoshield/signatures.ydb` (ExoFS)
- Format : `signatures/database.rs` — entrées indexées par hash
- Matcher YARA simplifié : patterns byte + wildcards
- Scan initial de tous les binaires Ring1 au démarrage
- Résultats → `engine/core` (threat records)

### 6.3 — Policy IPC Gate en Production [P0]

- `ipc_gate/policy.rs` : table de politiques persistée dans ExoFS
- Politiques par défaut chargées au démarrage
- Audit ring buffer : 4096 entrées circulaires
- `exo audit` interroge ce buffer + ExoLedger
- Toute violation → ExoLedger (chaîne BLAKE3)

### 6.4 — Sandbox Automatique pour `exo compat` [P0]

- `sandbox/container.rs` : sandbox appliquée à chaque processus Ring3 POSIX
- `sandbox/syscall_filter.rs` : allowlist syscalls par manifest capability
- `sandbox/fs_restriction.rs` : accès FS limités aux paths autorisés
- `sandbox/net_isolation.rs` : réseau autorisé/refusé par capability
- Manifest généré par `exo-pkg` au moment de l'installation

### 6.5 — Détection Réseau Active [P1]

- `network/firewall.rs` : règles stateful par default-deny
- `network/ids.rs` : détection patterns d'attaque réseau
- `network/dns_guard.rs` : filtrage DNS, blocage domaines suspects
- `network/traffic_analysis.rs` : anomalie trafic

### 6.6 — ML Inference Statique [P2]

- Modèle v0 embarqué dans le binaire (pas de training en prod)
- `ml/inference.rs` : inférence sur features comportementales
- `ml/features.rs` : extraction features depuis events
- Seuil de confiance configurable (default : HIGH uniquement)
- Pas de mise à jour modèle en v0.2.0

### 6.7 — ExoShield PhoenixSafe [P0]

- `on_pre_switch()` : flush alert buffer, sauvegarder état scoring
- `on_post_switch()` : recharger signatures, réabonner aux hooks
- Test : bascule ExoPhoenix → 0 événement perdu, 0 process non surveillé

---

## PHASE 7 — Installeur `exo` & PKG

### 7.1 — exo-pkg Binaire [P1]

- `exo install <pkg>` : résolution + download + vérif sig + injection ExoFS
- `exo compat install <pkg>` : idem + manifest capability + sandbox ExoShield
- `exo remove <pkg>` : révocation caps + nettoyage ExoFS
- `exo list`, `exo doctor`
- Affichage capabilities requises avant confirmation

### 7.2 — Milestone : `exo compat install calendar` [MILESTONE]

**Critère :** `exo compat run calendar` → calendrier texte affiché dans exosh.

### 7.3 — Milestone : `exo compat install curl` [MILESTONE]

**Critère :** `curl https://example.com` depuis exosh → 200 OK.

---

## PHASE 8 — Bootloader UEFI GPT Complet

### 8.1 — GPT Reader dans exo-boot [P1]

- Lecture GPT header (LBA 1) + backup (LBA -1)
- Validation CRC32 header + partition table
- Enumération des partitions (GUID type, start/end LBA)
- GUID types reconnus : ESP (C12A7328-...), ExoFS (custom UUID)
- Transmission des adresses partitions ExoFS dans BootInfo v2

### 8.2 — BootInfo v2 : Champs Strata [P1]

**Nouveaux champs vs BootInfo v1 :**
```rust
pub struct BootInfoV2 {
    // ... champs v1 existants ...
    pub exofs_root_phys: u64,      // Base physique partition ExoFS ROOT
    pub exofs_root_size: u64,      // Taille en octets
    pub exofs_data_phys: u64,      // Base physique partition ExoFS DATA
    pub exofs_data_size: u64,
    pub boot_partition_guid: [u8; 16],  // GUID de la partition d'où on a booté
    pub boot_info_version: u32,    // = 2 pour Strata
    pub _reserved: [u8; 28],
}
```

### 8.3 — Entrée NVRAM UEFI [P1]

- `EFI_BOOT_MANAGER_PROTOCOL` : création entrée BootXXXX au premier boot
- Path : `EFI/EXOOS/BOOTX64.EFI`
- Description : `ExoOS v0.2.0 — Strata`
- Booté en premier si Secure Boot désactivé ou clé ExoOS enregistrée

### 8.4 — Boot USB [P1]

- Détection ESP sur périphérique USB (EFI_SIMPLE_FILE_SYSTEM sur USB)
- Même chemin : `EFI/EXOOS/BOOTX64.EFI` + `kernel.elf`
- Usage : installation bare metal, rescue
- BootInfo v2 signale `boot_from_usb = true`

### 8.5 — Retrait GRUB [P2]

- `bootloader/grub.cfg` archivé, non utilisé en production
- Build system génère directement l'image `.img` avec GPT + ESP + ExoFS
- `make iso-strata` → image UEFI bootable

---

## PHASE 9 — USB Transfer Pipeline + Audio Système

### 9.1 — Pipeline USB → ExoFS [P0]

```
Clé USB insérée
  → usb_hid driver : USB_MASS_STORAGE_ATTACHED event
  → device_server : DEVICE_ATTACHED IPC
  → vfs_server : probe_mount()
      ├─ FAT32 détecté → fat_server → /mnt/usb
      └─ ExoFS détecté → vfs_server natif → /mnt/usb
  → exosh : `exo ls /mnt/usb` disponible
  → exo_shield : scan automatique de la clé (SCAN_REQUEST)
  → `exo cp /mnt/usb/file /apps/` → ExoFS + audit ExoLedger
  → `exo umount /mnt/usb` → flush + sync + détachement propre
```

**Commandes exosh associées :**
```
exo mount /dev/usb0 /mnt/usb        # manuel (auto si hot-plug)
exo ls /mnt/usb                     # format capability natif
exo cp /mnt/usb/app.elf /apps/      # transfer + audit
exo hash /mnt/usb/app.elf           # vérifie hash avant copie
exo umount /mnt/usb                 # éjection propre
```

**Audit ExoLedger sur chaque transfert :**
```
[2026-..] USB_TRANSFER src=/mnt/usb/app.elf dst=/apps/app.elf
          hash=9e4a72f1... size=2.1MiB pid=42 cap=@3d8f
```

### 9.2 — Audio Système : Chime + Bell + Alert [P1]

**Son de démarrage (boot chime) :**
- Déclenché par `init_server` après confirmation Vague 6 (exosh prêt)
- PCM statique embarqué dans `audio_server` : ~0.5s, 44100Hz, stéréo
- IPC : `audio_server::PLAY_SYSTEM_SOUND(SOUND_BOOT_COMPLETE)`
- Si audio_server indisponible : silent fallback (pas de panic)

**Terminal bell :**
- Déclenché par `tty_server` sur caractère BEL (0x07)
- IPC : `audio_server::BEEP(800, 100)` — 800Hz, 100ms
- Utilisé par exosh sur erreur, commande invalide, autocomplétion

**Alerte ExoShield :**
- Déclenché par exo_shield sur THREAT_LEVEL_HIGH ou THREAT_LEVEL_CRITICAL
- Son distinct : `audio_server::PLAY_SYSTEM_SOUND(SOUND_SECURITY_ALERT)`
- 3 bips courts pour HIGH, 1 bip long pour CRITICAL
- Ne peut pas être silencé par un processus Ring3

---

## PHASE 10 — Shell & Framebuffer

### 10.1 — fb_server Stable [P1]

- Framebuffer GOP UEFI opérationnel
- Blit depuis SHM Ring3 fonctionnel
- Événements input_server → fb_server → Ring3 routés
- Double buffering (pas de tearing)

### 10.2 — Shell Texte exosh v0.2.0 [P1]

- `exosh` accessible via TTY (pas de dépendance réseau — B-10 corrigé)
- Prompt `$ ` visible et interactif
- Commandes de base : `exo ls`, `exo cp`, `exo install`, `exo mount`, `exo umount`
- Format affichage capability natif (pas de rwx)
- Pas de crash sur entrée invalide
- Bell sur erreur (via audio_server)

---

## PHASE 11 — Observabilité & Qualité

### 11.1 — monitor_server [P2]

- Réception logs depuis tous serveurs Ring1
- Persistance dans ExoFS
- `exo log` avec filtres
- `exo metrics` : CPU, mémoire, IPC, réseau, audio

### 11.2 — Suite Tests Sécurité Complète [P0]

```
security_test::exoseal_verify_chain             PASS
security_test::exocage_all_mechanisms           PASS
security_test::zerotrust_ipc_blocked            PASS
security_test::captoken_access_denied           PASS
security_test::captoken_revocation_immediate    PASS
security_test::captoken_no_privilege_escalation PASS
security_test::exokairos_throttle_at_100pct     PASS
security_test::exokairos_kill_at_200pct         PASS
security_test::exoledger_chain_integrity        PASS
security_test::exoledger_immutable              PASS
security_test::exoshield_dma_fault              PASS
security_test::exonmi_watchdog_fires            PASS
security_test::exoshield_sandbox_escape_blocked PASS  ← nouveau Strata
```

---

## PHASE 12 — Validation Release Strata

### Critères de Release

| Critère | Seuil | Méthode |
|---|---|---|
| Kernel stability | ≥ 98% | Stress test 2h+ sans crash |
| ExoPhoenix | 100% | phoenix_stress_test (1000 bascules) |
| Sécurité | 13/13 PASS | security_integration_tests |
| musl-exo syscalls | ≥ 127 | musl_exo_test suite |
| `calendar` POSIX | Fonctionnel | Test manuel exosh |
| `curl https` POSIX | Fonctionnel | Test manuel exosh |
| USB transfer | Fonctionnel | Clé FAT32 + ExoFS |
| Boot chime | Joué | Boot QEMU + hardware |
| UEFI natif | Fonctionnel | QEMU OVMF + hardware |
| AHCI ou NVMe | ≥ 1 opérationnel | QEMU + hardware |
| Tests unitaires | 100% PASS | `cargo test --all` |
| Tests intégration | 100% PASS | `cargo test --test integration` |
| ExoLedger integrity | 0 rupture | `exo audit --verify-chain` |
| Memory leaks (2h) | 0 | Balloon test |
| IPC drops (1h) | 0 | monitor_server metrics |

### Checklist Finale

- [ ] Tous les P0 résolus
- [ ] MASTER-CHECKLIST-STRATA : 100%
- [ ] CORR series à jour (CORR-87+)
- [ ] `VISION-STRATA.md` — tous piliers validés
- [ ] `exo doctor` → 0 erreur critique
- [ ] Git tag : `v0.2.0-rc1` puis `v0.2.0-strata`

---

## Graphe de Dépendances

```
[BLOC-1 bugs] ─────────────────────────────────────────► tout
[BLOC-0 outillage] ─────────────────────────────────────► tout

[Ph0: exo-alloc + TLS + SSR] ──────────────────────────┐
[Ph1: sécurité chain] ──────────────────────────────────┤
[Ph2: crypto + réseau] ─────────────────────────────┐   │
[Ph3: ExoFS + vfs] ─────────────────────────────┐   │   │
[Ph4: fork/CoW + musl-exo] ─────────────────┐   │   │   │
[Ph5: AHCI/NVMe/USB/audio] ──────────────┐  │   │   │   │
[Ph6: ExoShield Ring1] ──────────────┐   │  │   │   │   │
                                      │   │  │   │   │   │
[Ph7: exo-pkg] ◄──────────────────────┘   │  │   │   │   │
    │                                      │  │   │   │   │
    ▼                                      │  │   │   │   │
[calendar] [curl] ◄───────────────────────┘  │   │   │   │
                                              │   │   │   │
[Ph8: bootloader GPT] ◄───────────────────────┘   │   │   │
[Ph9: USB pipeline + audio] ◄─────────────────────┘   │   │
[Ph10: exosh + fb_server] ◄───────────────────────────┘   │
[Ph11: observabilité + tests] ◄───────────────────────────┘
[Ph12: Release Strata]
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — ROADMAP-STRATA.md*
