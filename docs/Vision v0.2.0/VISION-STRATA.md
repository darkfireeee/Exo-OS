# ExoOS v0.2.0 — Strata
## Vision & Périmètre Officiel

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** FONDATEUR — remplace et invalide VISION-V0.2.0.md
**Nom de code :** Strata

---

## 1. Déclaration d'Intention

**Strata** est la release de consolidation structurelle d'ExoOS.

Le nom est géologique : les strates sont les couches que l'on ne voit pas mais sur lesquelles tout repose. Strata ne cherche pas à impressionner visuellement — elle construit l'irréversible. À la fin de cette release, ExoOS est un **ordinateur-serveur complet et autonome** : il démarre depuis un vrai firmware UEFI, monte ses partitions, gère son stockage physique, sécurise chaque accès, transfère des fichiers depuis une clé USB, joue un son au démarrage, et exécute des programmes POSIX dans une sandbox de capabilities.

Il n'y a pas d'image. Il n't y a pas de bureau. Mais tout ce qu'un ordinateur fait pour fonctionner — vraiment fonctionner — est présent.

La v0.3.0 lui donnera un visage. Strata lui donne une âme.

---

## 2. Ce que v0.2.0 — Strata Est

Un **ordinateur headless entièrement fonctionnel** :

```
Démarrage UEFI natif
    ↓
GPT lu → ESP monté → kernel vérifié Ed25519 → chargé + KASLR
    ↓
Boot 9 phases → sécurité complète activée
    ↓
Ring1 par vagues : memory / crypto / device / vfs / network / shield / tty / exosh
    ↓
Son de démarrage (HDA ou virtio_sound)
    ↓
exosh disponible → prompt interactif
    ↓
Clé USB insérée → montée dans ExoFS → transfert de fichiers
    ↓
exo compat install calendar → POSIX exécuté dans sandbox capabilities
    ↓
ExoShield surveille tout : syscalls, mémoire, réseau, comportement
    ↓
ExoLedger audite tout : chaîne BLAKE3 inaltérable
```

---

## 3. Ce que v0.2.0 — Strata N'Est PAS

| Hors périmètre | Pourquoi | Version cible |
|---|---|---|
| Wayland / compositeur | Pas de serveur graphique | v0.3.0 |
| Applications GUI | Dépendent de Wayland | v0.3.0+ |
| wgpu / winit / iced | Stack graphique non disponible | v0.3.0 |
| Lecture multimédia (VLC, musique) | Dépend de Wayland + mixer | v0.3.0 |
| Audio multi-application / mixer | Dépend de compositeur | v0.3.0 |
| D-Bus / zbus | Incompatible IPC ExoOS | Jamais |
| PAM / shadow / systemd / launchd | Incompatibles modèle capability | Jamais |
| `apt install X` | Format incorrect pour ExoOS | Jamais |
| tokio (runtime) | Remplacé par exo-runtime | Jamais |

---

## 4. Les Six Piliers de Strata

### Pilier 1 — Kernel à ~98% de Maturité

**Objectif :** Chaque sous-système est fonctionnel, stable, testé. Zéro P0 ouvert, zéro deadlock connu, zéro memory leak sur stress 2h+.

| Sous-système | État v0.1.x | Cible Strata |
|---|---|---|
| Mémoire (buddy, SLUB, vmalloc, CoW, swap) | ~85% | 98% |
| Scheduler (CFS, RT, deadline, SMP, FPU) | ~80% | 98% |
| IPC (SpscRing, sync, SHM, RPC) | ~82% | 98% |
| Process (fork, exec, signal, wait, thread) | ~75% | 98% |
| FS (ExoFS + VFS bridge) | ~82% | 98% |
| Sécurité (capability, zero-trust, isolation) | ~70% | 98% |
| Drivers (PCI, virtio, USB, AHCI, NVMe, audio) | ~40% | 90% |
| ExoPhoenix (dual-kernel, SSR, resurrection) | ~90% | 100% |

Les bugs bloquants BLOC-1 (CORR-76 à CORR-86) sont des prérequis absolus :
VirtIO BAR hardcodé, ExoFS non-persistant, boot -m 2G, ELF base 0x400000, etc.

---

### Pilier 2 — Chaîne de Sécurité Entièrement Active

**Objectif :** Chaque composant de sécurité est actif en production. ExoShield n'est pas un stub — c'est un serveur Ring1 complet, le dernier démarré dans la chaîne Ring1 (Phase 3), qui surveille en temps réel l'intégralité du système.

#### 2.1 — Chaîne de Boot Sécurité

```
ExoSeal  →  ExoCage  →  ZeroTrust  →  CapToken
    →  ExoKairos  →  ExoLedger  →  ExoShield  →  ExoNMI
```

Chaque composant doit être **actif**, pas juste initialisé.

#### 2.2 — ExoShield : Serveur EDR Complet (Ring1, Phase 3)

ExoShield est un serveur de sécurité de niveau EDR (Endpoint Detection & Response). Ses modules sont opérationnels dès le démarrage :

| Module | Rôle | Niveau v0.2.0 |
|---|---|---|
| `engine/core` | Scoring de menaces, profils de risque, records | Actif |
| `engine/scanner` | Signatures YARA, heuristiques, scan périodique | Actif |
| `engine/realtime` | Monitoring temps réel, rate tracking, alertes | Actif |
| `behavioral/` | Détection d'anomalies, profilage, séquences | Actif |
| `hooks/` | Hooks syscall, exec, mémoire, réseau | Actifs |
| `ipc_gate/` | Policy enforcement, audit ring buffer | Actif |
| `signatures/` | Base YARA chargée depuis ExoFS au boot | Actif |
| `sandbox/` | Isolation containers pour `exo compat` | Actif |
| `network/` | DNS guard, firewall, IDS, analyse trafic | Actif |
| `ml/` | Inférence sur modèle statique embarqué | Actif — modèle v0 |
| `forensics/` | Dump mémoire, timeline, rapport | Sur demande |

**IPC Protocol (7 types) :**
- `SCAN_REQUEST(0)` — scan processus/région mémoire
- `EVENT_REPORT(1)` — événement sécurité pour analyse temps réel
- `QUARANTINE_CMD(2)` — contenir/libérer un processus
- `THREAT_QUERY(3)` — consulter les enregistrements de menaces
- `POLICY_UPDATE(4)` — mettre à jour les politiques/filtres
- `HEARTBEAT(5)` — contrôle de vivacité
- `PMC_ANOMALY(6)` — rapport d'anomalie hardware counter

**Règle PhoenixSafe :** ExoShield implémente `on_pre_switch()` et `on_post_switch()` pour une bascule ExoPhoenix sans perte d'état.

---

### Pilier 3 — ExoPhoenix Parfait

**Objectif :** Bascule A↔B reproductible < 500ms, zéro perte de capabilities survivantes, 1000 bascules sans échec.

Voir `SPEC-EXOPHOENIX-STRATA.md` pour la spécification complète.

Points critiques :
- SSR bitmask `[u64; CORE_MASK_WORDS]` pour 256 cores (CORR-75)
- `SSR_MAX_PROCESSES = 24` avec politique de priorisation
- SSR région physique `[0x0100_0000..0x0110_0000)` — 64 KiB, exclue des allocateurs
- Ring1 démarré en parallèle après bascule (pas en chaîne séquentielle)

---

### Pilier 4 — Exécution POSIX via `exo compat`

**Objectif :** Installer et exécuter des programmes POSIX en mode texte (`calendar`, `vim`, `curl`). ExoShield sandbox chaque processus compat automatiquement.

```
$ exo compat install calendar
[exo-pkg] Résolution bundle...
[crypto_server] Vérification signature...
[exo_shield] Manifest capabilities généré: 12 syscalls, 3 FS paths, 0 réseau
$ exo compat run calendar
```

La pile complète : `exo-alloc → musl-exo (127 syscalls) → vfs_server → crypto_server → network_server → exo-pkg → sandbox ExoShield`

---

### Pilier 5 — Bootloader UEFI Réel + Schéma de Partitions

**Objectif :** ExoOS démarre depuis un vrai firmware UEFI sur matériel physique ou VM, sans GRUB, avec un schéma de partitions GPT propre.

#### Schéma de partitions ExoOS v0.2.0

```
┌─────────────────────────────────────────────────────┐
│  GPT Header (bloc 1)                                │
├─────────────────────────────────────────────────────┤
│  Partition 1 : ESP (FAT32)           ~256 MB        │
│    EFI/EXOOS/BOOTX64.EFI  ← exo-boot signé         │
│    EFI/EXOOS/kernel.elf   ← kernel signé Ed25519   │
│    EFI/EXOOS/exo-boot.cfg                           │
├─────────────────────────────────────────────────────┤
│  Partition 2 : ExoFS ROOT            ~4 GB min      │
│    /servers/  ring1 servers                         │
│    /lib/      musl-exo, exo-crates                  │
│    /bin/      exosh, exo, outils système            │
│    /etc/      config, ExoShield policy, clés        │
│    /var/      ExoLedger (sealed), logs              │
├─────────────────────────────────────────────────────┤
│  Partition 3 : ExoFS DATA            reste          │
│    /home/     données utilisateur                   │
│    /apps/     packages installés via exo            │
│    /tmp/      temporaire (epoch-cleared au boot)    │
└─────────────────────────────────────────────────────┘
```

**Entrée NVRAM UEFI :** `EFI/EXOOS/BOOTX64.EFI` enregistrée dans la variable `BootXXXX` NVRAM au premier boot (via `EFI_BOOT_MANAGER_PROTOCOL`).

**Boot USB :** exo-boot détecte et démarre depuis une clé USB formatée avec le même schéma (ESP + ExoFS ROOT). Utilisé pour installation et rescue.

**Ce que le bootloader transmet dans BootInfo v2 :**
- Carte mémoire UEFI complète
- Adresse physique + taille de chaque partition ExoFS
- Framebuffer GOP (résolution, stride, format)
- ACPI RSDP
- KASLR offset appliqué
- 64 octets d'entropie (`EFI_RNG_PROTOCOL`)
- Statut Secure Boot

---

### Pilier 6 — Hardware Complet : USB, Storage, Audio

**Objectif :** ExoOS exploite le matériel réel. Pas de simulation, pas de virtio exclusif — le métal nu répond.

#### 6.1 — Transferts USB via ExoFS

```
Clé USB insérée (physique ou QEMU usb-storage)
  → USB HID driver : détecte USB Mass Storage (BBB protocol)
  → device_server : émet événement DEVICE_ATTACHED
  → vfs_server : monte automatiquement selon détection
      ├─ Clé FAT32 → fat_server → mount /mnt/usb
      └─ Clé ExoFS → exofs_server natif → mount /mnt/usb
  → exosh : `exo ls /mnt/usb` affiché en format capability natif
  → `exo cp /mnt/usb/app.elf /apps/` → transfert + audit ExoLedger
  → `exo umount /mnt/usb` → ejection propre
```

ExoShield applique automatiquement une politique de scan à tout fichier transféré depuis USB.

#### 6.2 — Storage Bare Metal

| Driver | Cible | Usage |
|---|---|---|
| `virtio_blk` | ✅ Opérationnel | QEMU/VM |
| `ahci` | Strata | SATA bare metal (serveur, PC) |
| `nvme` | Strata | SSD NVMe bare metal |
| `usb_hid` | Strata | USB Mass Storage + HID |

#### 6.3 — Audio Système (Non-Multimédia)

L'audio v0.2.0 n'est pas un lecteur multimédia — c'est la voix du système. Trois usages précis :

| Événement | Son | Implémentation |
|---|---|---|
| **Boot chime** | PCM court (~0.5s) au démarrage Ring1 complet | `audio_server` → HDA ou virtio_sound |
| **Terminal bell** | Beep sur erreur exosh, commande invalide | `tty_server` → `audio_server` IPC |
| **Alerte sécurité** | Ton distinct sur événement critique ExoShield | `exo_shield` → `audio_server` IPC |

Un son au démarrage confirme que le système est vivant. Un son d'alerte confirme que la sécurité surveille. C'est la signature sonore d'un vrai ordinateur.

**Drivers audio v0.2.0 :**
- `audio/hda` — Intel HD Audio (hardware physique)
- `audio/virtio_sound` — virtio-snd (QEMU/VM)

**Hors périmètre audio v0.2.0 :** mixer multi-application, lecture fichiers audio, API Ring3 audio générique. Ces fonctions attendent v0.3.0.

---

## 5. Architecture des Composants — Vue Strata

```
╔══════════════════════════════════════════════════════════════════════╗
║                    RING 3 — APPLICATIONS & LIBS                     ║
║                                                                      ║
║  exo-alloc  exo-net  exo-crypto  exo-fs  exo-runtime  exo-pkg      ║
║  ┌────────────────────────────────────────────────────────────────┐  ║
║  │           musl-exo + exo-libc (POSIX compat, 127 syscalls)   │  ║
║  └────────────────────────────────────────────────────────────────┘  ║
║  exosh  exo-pkg  exo-observability                                   ║
╠══════════════════════════════════════════════════════════════════════╣
║                    RING 1 — SERVEURS SYSTÈME                         ║
║                                                                      ║
║  Vague 1 : memory_server  scheduler_server  crypto_server           ║
║  Vague 2 : device_server  virtio_blk  ahci  nvme  usb_hid           ║
║  Vague 3 : vfs_server (ExoFS + FAT + ext4)                          ║
║  Vague 4 : tty_server  input_server  network_server  audio_server   ║
║  Vague 5 : exo_shield (EDR complet — dernier, voit tout)            ║
║  Vague 6 : exosh                                                     ║
╠══════════════════════════════════════════════════════════════════════╣
║                    RING 0 — KERNEL                                   ║
║                                                                      ║
║  memory/  scheduler/  ipc/  security/  exophoenix/  fs/  drivers/  ║
╠══════════════════════════════════════════════════════════════════════╣
║                    UEFI FIRMWARE                                      ║
║                                                                      ║
║  exo-boot (BOOTX64.EFI) — GPT → ESP → kernel.elf → BootInfo v2     ║
╚══════════════════════════════════════════════════════════════════════╝
```

---

## 6. Ordre de Priorité Absolu

```
 P0 — BLOC -1 : bugs bloquants kernel (CORR-76..86)
 P0 — BLOC 0  : outillage audit (const_assert, semgrep, cargo deny)

 Phase 0 : exo-alloc + generic-rt TLS + SSR bitmask fix
 Phase 1 : chaîne sécurité complète (ExoSeal→ExoNMI)
 Phase 2 : crypto_server + network_server + rustls
 Phase 3 : ExoFS fsck + vfs_server natif + fat_server
 Phase 4 : fork/CoW fixes + musl-exo 127 syscalls
 Phase 5 : drivers AHCI + NVMe + USB HID + audio
 Phase 6 : ExoShield intégration complète Ring1
 Phase 7 : exo-pkg + milestones calendar/curl
 Phase 8 : bootloader UEFI GPT complet
 Phase 9 : USB transfer pipeline + audio chime
 Phase 10 : fb_server stable + exosh texte
 Phase 11 : observabilité + tests sécurité 13/13
 Phase 12 : validation release Strata
```

---

## 7. Identité de la Release

**ExoOS v0.2.0 — Strata**

> Un ordinateur qui fonctionne. Sans image, mais avec tout ce qu'un ordinateur fait pour fonctionner : démarrer depuis le firmware, monter ses partitions, sécuriser chaque accès, transférer des fichiers depuis une clé USB, jouer un son, et exécuter des programmes. Les strates ne se voient pas — elles portent tout le reste.

**Métriques de succès Strata :**

| Métrique | Cible |
|---|---|
| Stabilité kernel | ≥ 98% (stress 2h+) |
| ExoPhoenix recovery | < 500ms (1000 bascules) |
| Tests sécurité ExoShield | 13/13 PASS |
| Syscalls POSIX musl-exo | ≥ 127 |
| `calendar` via `exo compat` | Fonctionnel |
| `curl https://` via `exo compat` | Fonctionnel |
| Transfert clé USB → ExoFS | Fonctionnel |
| Boot chime | Joué au démarrage |
| Boot UEFI natif (sans GRUB) | Fonctionnel |
| AHCI ou NVMe bare metal | Au moins un opérationnel |
| Tests unitaires | 100% PASS |
| MASTER-CHECKLIST-STRATA | 100% |

---

## 8. Vision v0.3.0 — Pour Référence

**ExoOS v0.3.0** aura un visage :
- Compositeur Wayland natif ExoOS
- Accélération GPU (DRM/KMS + wgpu hardware backend)
- Shell graphique `iced` + `exosh` GUI
- Notifications visuelles
- Lecteur multimédia (VLC via `exo compat install vlc`)
- Audio multi-application avec mixer Ring1
- Gestionnaire de mises à jour avec UI

Strata pose tout ce sur quoi v0.3.0 s'appuiera.

---

*claude-alpha — ExoOS v0.2.0 — Strata — VISION-STRATA.md*
