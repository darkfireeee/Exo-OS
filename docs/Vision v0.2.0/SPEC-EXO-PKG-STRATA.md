# SPEC-EXO-PKG-STRATA — Gestionnaire de Paquets ExoOS
## Commande `exo` · Format .exo-bundle · Registre

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXO-PKG.md

---

## 1. Principe Fondamental

`apt install`, `pacman -S`, `dnf install` sont des outils Linux. Ils supposent `/usr`, `/bin`, `/lib`, permissions `rwx`+`uid/gid`, un daemon de fond, l'absence de capabilities.

**ExoOS utilise `exo`**, conçu autour de :
- ExoFS (objets, capabilities, epochs, content-addressed)
- Signature cryptographique via `crypto_server` (Ed25519 + BLAKE3)
- Sandboxes de capabilities par application (ExoShield)
- Installation sans élévation de privilège globale

---

## 2. Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                     exo  (binaire Ring3)                       │
│                                                                │
│  exo install   exo compat   exo cap   exo phoenix  exo doctor │
│       │              │          │          │             │     │
└───────┼──────────────┼──────────┼──────────┼─────────────┼────┘
        │              │          │          │             │
   ExoFS bundle   musl-exo    security    ExoPhoenix   monitor
   (natif)        sandbox     _server     IPC          _server
```

---

## 3. Les Deux Modes d'Installation

### Mode 1 : `exo install` — Applications Natives ExoOS

```bash
exo install exo-calendar
exo install exo-texteditor
exo install exo-monitor@0.2.1
```

**Séquence interne :**
```
[1] Résolution : registre ExoOS → bundle.exo-bundle
[2] Vérification signature : crypto_server (Ed25519 + BLAKE3 manifest)
[3] Lecture manifest : { binaire, capabilities_requises, deps }
[4] Approbation : affichage capabilities demandées → confirmation utilisateur
[5] Injection ExoFS : blob → /apps/exo-calendar/
[6] Enregistrement caps : security_server octroie les capabilities
[7] Index : exo-index.exo mis à jour (epoch++, hash new)
[8] ExoLedger : INSTALL event (hash, caps, source)
```

**Manifest d'app native (.exo-manifest) :**
```toml
[package]
name      = "exo-calendar"
version   = "1.0.0"
author    = "ExoOS Project"
signature = "ed25519:9f3c7a1e..."

[binary]
path = "bin/exo-calendar"
type = "elf-exo"  # ELF compilé pour ExoOS (exo-alloc + exo-runtime)

[capabilities.required]
fs  = { rights = "rw-l", scope = "~/.calendar/" }
time = { rights = "r", scope = "system_clock" }
ipc = { rights = "send", scope = "notification_server" }

[capabilities.optional]
net = { rights = "r", scope = "*.caldav.example.com" }

[resources]
memory_max        = "64MiB"
cpu_budget_ms_per_sec = 100   # ExoKairos budget

[exophoenix]
restore_mode = "Restart"      # Restart / Resume / Abandon
```

### Mode 2 : `exo compat install` — Applications POSIX

```bash
# Apps text-mode fonctionnelles en Strata (headless)
exo compat install calendar
exo compat install vim
exo compat install curl
exo compat install htop
exo compat install python3
exo compat install git
exo compat install ssh

# Apps GUI — s'installent avec avertissement (rendu v0.3.0)
exo compat install vlc      # ⚠ GPU absent — text fallback si disponible
exo compat install firefox  # ⚠ Wayland absent — v0.3.0 requis
```

**Séquence interne (compat) :**
```
[1] Résolution : registre compat ExoOS → bundle.exo-bundle
[2] Vérification signature : crypto_server
[3] Génération manifest capabilities :
    analyse statique syscalls + dépendances du binaire
    (via exo_shield::sandbox::analyze_binary)
[4] Affichage : "calendar nécessite : read/write FS, terminal, clock — réseau : NONE"
[5] Confirmation utilisateur
[6] Injection ExoFS : → /compat/calendar/
[7] Sandbox ExoShield configurée avec manifest généré
[8] ExoLedger : COMPAT_INSTALL event
```

---

## 4. Interface Complète de la Commande `exo`

```bash
# Installation
exo install <pkg>[@version]           # App native ExoOS
exo compat install <pkg>[@version]    # App POSIX dans sandbox
exo remove <pkg>                      # Désinstaller + révoquer caps
exo update                            # Mettre à jour tous les paquets
exo update <pkg>                      # Mettre à jour un paquet
exo info <pkg>                        # Manifest, caps, hash, dépendances

# Recherche et registre
exo search <terme>
exo registry add <url>
exo registry list
exo registry trust <url> <key>

# Capabilities
exo cap list [--pid <pid>]            # Lister caps d'un process
exo cap grant <pkg> <cap>             # Octroyer une cap supplémentaire
exo cap revoke <pkg> <cap>            # Révoquer
exo cap request <cap> <scope>         # Demander interactivement

# Filesystem & USB (Strata)
exo ls [path]                         # Lister en format capability natif
exo cp <src> <dst>                    # Copier + audit ExoLedger
exo mv <src> <dst>                    # Déplacer O(1) dans ExoFS
exo hash <file>                       # BLAKE3 d'un fichier
exo verify <file>                     # Vérifier signature Ed25519
exo mount <dev> <mountpoint>          # Monter un volume
exo umount <mountpoint>               # Éjecter proprement

# POSIX compat
exo compat run <pkg> [args]           # Exécuter app POSIX
exo compat shell                      # Shell POSIX dans sandbox (debug)
exo compat list-syscalls <pkg>        # Lister syscalls utilisés

# Système
exo doctor                            # Diagnostic système complet
exo audit [--verify-chain]            # ExoLedger — dernières entrées
exo phoenix status                    # État ExoPhoenix A/B
exo phoenix switch                    # Bascule manuelle
exo metrics                           # CPU, mémoire, IPC, réseau, audio
exo log [--filter <component>]        # Logs monitor_server
```

---

## 5. Format de Paquet `.exo-bundle`

Un bundle ExoOS est un objet ExoFS composite :

```
bundle.exo-bundle
├── manifest.toml          # Métadonnées + capabilities requises
├── signature.ed25519      # Signature Ed25519 du manifest + contenu
├── bin/
│   └── <nom>             # Binaire ELF Ring3
├── lib/
│   └── *.so              # Libs dynamiques (optionnel)
├── assets/               # Ressources statiques
└── checksums.blake3       # BLAKE3 de chaque fichier du bundle
```

La signature couvre l'ensemble (manifest + contenu). Vérification via `crypto_server`.

---

## 6. Registre ExoOS

```
https://registry.exoos.dev/
    index.exo                    # Index signé (BLAKE3 + Ed25519)
    packages/
        exo-calendar/
            1.0.0.exo-bundle
            1.0.0.exo-bundle.sig
    compat/
        calendar/
            2.11.1.exo-bundle    # Repackagé depuis Debian/Alpine
        vim/
            9.1.0.exo-bundle
        curl/
            8.7.0.exo-bundle
        htop/
            3.3.0.exo-bundle
        python3/
            3.12.0.exo-bundle
```

**Miroir compat :** Repackage les binaires Debian/Alpine avec génération automatique du manifest de capabilities (analyse statique syscalls + dépendances).

---

## 7. Arborescence ExoFS — Apps Installées

```
/sys/           # Objets kernel (caps, config)
/srv/           # Serveurs Ring1
/apps/          # Applications natives ExoOS
    exo-calendar/
        bin/exo-calendar      [x  r-x---- [✓sig] @3d8f  ep:0038]
        config.exo            [c  rw----- ·      @2b9d  ep:0041]
        data/                 [d  rwxl--- ·      @9f3c  ep:0042]
/compat/        # Applications POSIX (sandbox musl-exo)
    calendar/
        bin/calendar          [x  r-x---- [✓sig] @8a2c  ep:0039]
        lib/libc.so           [b  r------ [✓sig] @1f7b  ep:0039]
        _sandbox/             [d  rwxl--- [enc]  @4e9d  ep:0039]
/home/
    eric/
        documents/            [d  rwxl---  ·      @9f3c  ep:0042]
/mnt/
    usb/                      ← Montage clé USB (hot-plug)
```

---

## 8. Désinstallation

```bash
exo remove calendar
```

```
[1] Révocation de toutes les caps liées au paquet (security_server)
[2] Suppression objets ExoFS : /compat/calendar/ (refcount--)
[3] Si refcount == 0 : GC ExoFS récupère l'espace (epoch GC)
[4] Mise à jour index
[5] ExoLedger : UNINSTALL { pkg: calendar, epoch: 43, revoked_caps: [...] }
```

---

## 9. `exo doctor` — Diagnostic Système

```
$ exo doctor

ExoOS v0.2.0 — Strata — Diagnostic Système
═══════════════════════════════════════════════════════

[✓]  Kernel stability          98.2%    (≥98% requis)
[✓]  ExoPhoenix                kernel-A  ep:43  bascules:3
[✓]  security_chain            27/27 composants actifs
[✓]  ExoShield Ring1           running   pid:17  threats:0
[✓]  ExoLedger                 ep:43     4422 entries  ✓chain
[✓]  crypto_server             running   pid:4
[✓]  vfs_server                running   pid:11  ExoFS ep:43
[✓]  network_server            running   pid:15  192.168.1.42/24
[✓]  device_server             running   pid:7   3 PCI, 1 USB
[✓]  audio_server              running   pid:16  HDA detected
[✓]  musl-exo                  127/127 syscalls P1+P2
[✓]  USB                       /dev/usb0 monté /mnt/usb (FAT32, 15.9 GiB)

[WARN]  nvme_server             aucun NVMe détecté (AHCI actif)
[INFO]  Packages                4 native, 3 compat (calendar, vim, curl)
[INFO]  exo-pkg registry        https://registry.exoos.dev  ✓  last-sync: 2h

Recommandations :
  - Aucun problème critique
  - NVMe non détecté : normal si QEMU sans NVMe
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-EXO-PKG-STRATA.md*
