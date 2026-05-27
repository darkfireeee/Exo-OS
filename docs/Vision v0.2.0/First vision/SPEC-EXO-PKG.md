# SPEC-EXO-PKG — Gestionnaire de Paquets ExoOS
## Commande `exo` et Système d'Installation

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0

---

## 1. Déclaration

`apt install`, `pacman -S`, `dnf install` sont des outils conçus pour Linux. Ils supposent :
- Un système de fichiers POSIX avec `/usr`, `/bin`, `/lib`
- Des permissions `rwx` + `uid/gid`
- Un daemon de fond (dpkg, rpm)
- L'absence de notion de capability

**ExoOS utilise `exo`**, son propre gestionnaire de paquets, conçu autour de :
- ExoFS (objets, capabilities, epochs)
- La signature cryptographique via `crypto_server`
- Les sandboxes de capabilities par application
- L'installation sans élévation de privilège globale

---

## 2. Architecture de `exo`

```
┌─────────────────────────────────────────────────────────────┐
│                    exo  (binaire Ring3)                     │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │  exo install │  │  exo compat  │  │  exo cap         │  │
│  │  (natif)     │  │  (POSIX)     │  │  (capabilities)  │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘  │
│         │                 │                    │             │
└─────────┼─────────────────┼────────────────────┼────────────┘
          │                 │                    │
    ExoFS bundle      musl-exo shim         security_server
    (objets natifs)   (POSIX sandbox)       (capability grant)
```

---

## 3. Les Deux Modes d'Installation

### Mode 1 : `exo install` — Applications Natives ExoOS

Pour les applications conçues pour ExoOS, utilisant l'IPC natif et les capabilities.

```bash
# Syntaxe
exo install <nom-paquet>[@version]

# Exemples
exo install exo-calendar
exo install exo-texteditor
exo install exo-browser@0.1.2  # quand disponible
```

**Processus interne :**
```
exo install exo-calendar
    │
    ├─[1] Résolution: registre ExoOS → bundle.exo
    ├─[2] Vérification signature: crypto_server (Ed25519 + BLAKE3)
    ├─[3] Lecture manifest: { binaire, capabilities_requises, deps }
    ├─[4] Approbation: affichage capabilities demandées → confirmation user
    ├─[5] Injection ExoFS: blob "exo-calendar" → /apps/exo-calendar/
    ├─[6] Enregistrement caps: security_server octroie les capabilities
    └─[7] Entrée index: exo-index.exo mis à jour (epoch++, hash new)
```

**Manifest d'une app native (format .exo-manifest) :**
```toml
[package]
name = "exo-calendar"
version = "1.0.0"
author = "ExoOS Project"
signature = "ed25519:9f3c7a1e..."

[binary]
path = "bin/exo-calendar"
type = "elf-exo"  # ELF compilé pour ExoOS (musl-exo + exo-runtime)

[capabilities.required]
fs = { rights = "rw-l", scope = "~/.calendar/" }
time = { rights = "r", scope = "system_clock" }
ipc = { rights = "send", scope = "notification_server" }

[capabilities.optional]
net = { rights = "r", scope = "*.caldav.example.com" }  # sync CalDAV

[resources]
memory_max = "64MiB"
cpu_budget_ms_per_sec = 100  # ExoKairos budget
```

### Mode 2 : `exo compat install` — Applications POSIX

Pour les applications conçues pour Linux/POSIX, exécutées dans une sandbox de compatibilité.

```bash
# Syntaxe
exo compat install <nom-paquet>[@version]

# Exemples v0.2.0 (utiles, text-mode)
exo compat install calendar   # agenda texte
exo compat install vim        # éditeur texte
exo compat install curl       # client HTTP CLI
exo compat install htop       # monitoring texte
exo compat install python3    # runtime Python

# Exemples théoriques v0.2.0 (installable, non affichable sans Wayland)
exo compat install vlc        # ← s'installe, AVERTIT que rendu GPU absent
exo compat install firefox    # ← s'installe, AVERTIT que rendu GPU absent
```

**Processus interne :**
```
exo compat install calendar
    │
    ├─[1] Résolution: miroir POSIX-compat → bundle POSIX
    ├─[2] Vérification signature + hash
    ├─[3] Analyse dépendances: { libc, libncurses, ... }
    ├─[4] Vérification couverture musl-exo
    │       ├─ OK: tous les syscalls couverts par musl-exo → go
    │       └─ WARN: syscalls non couverts → liste + avertissement
    ├─[5] Injection ExoFS: blobs POSIX → /compat/calendar/
    ├─[6] Sandbox POSIX: manifest généré automatiquement (capabilities minimales)
    ├─[7] Shims: musl-exo configuré pour ce processus
    └─[8] Test de lancement rapide: exécution de `--version` ou `--help`
```

**Avertissement pour apps graphiques (v0.2.0) :**
```
$ exo compat install vlc
[INFO]  Résolution: vlc 3.0.21 → 45.2 MiB
[INFO]  Vérification signature: ✓
[WARN]  Dépendances graphiques détectées:
          - libGL.so     → wgpu non disponible en v0.2.0
          - libwayland   → Wayland absent (prévu v0.3.0)
          - libXcb       → X11 absent
[INFO]  Installation en mode "théorique" :
          - Les binaires seront installés dans ExoFS
          - L'exécution graphique échouera avec EXO-0503
          - vlc --audio-only POURRAIT fonctionner (expérimental)
[?]  Continuer quand même? [o/N]: o
[INFO]  Installation... ✓
[INFO]  Pour lancer (audio uniquement, expérimental):
          exo compat run vlc --audio-only /chemin/fichier.mp3
```

---

## 4. Commandes Complètes

```bash
# Installation
exo install <pkg>              # App native ExoOS
exo compat install <pkg>       # App POSIX/Linux

# Gestion
exo list                       # Lister les apps installées
exo list --compat              # Lister les apps POSIX installées
exo remove <pkg>               # Désinstaller (avec révocation capabilities)
exo update                     # Mettre à jour tous les paquets
exo update <pkg>               # Mettre à jour un paquet spécifique
exo info <pkg>                 # Infos sur un paquet (manifest, caps, hash)
exo search <terme>             # Rechercher dans le registre

# Capabilities
exo cap list [--pid <pid>]     # Lister les capabilities d'un processus
exo cap grant <pkg> <cap>      # Octroyer une capability supplémentaire
exo cap revoke <pkg> <cap>     # Révoquer une capability
exo cap request <cap> <scope>  # Demander une capability (interactif)

# Sandboxes POSIX
exo compat run <pkg> [args]    # Exécuter une app POSIX
exo compat shell               # Shell POSIX dans une sandbox (debug)
exo compat list-syscalls <pkg> # Lister les syscalls utilisés

# Registres
exo registry add <url>         # Ajouter un registre
exo registry list              # Lister les registres configurés
exo registry trust <url> <key> # Faire confiance à un registre (clé Ed25519)

# Diagnostic
exo doctor                     # Vérification santé du système
exo audit                      # Afficher les dernières entrées ExoLedger
exo phoenix status             # État ExoPhoenix
```

---

## 5. Format de Paquet ExoOS (`.exo-bundle`)

Un bundle ExoOS est un objet ExoFS composite :

```
bundle.exo-bundle
├── manifest.toml          # Métadonnées et capabilities requises
├── signature.ed25519      # Signature Ed25519 du manifest + contenu
├── bin/
│   └── <nom>             # Binaire ELF (Ring3, musl-exo linked)
├── lib/
│   └── *.so              # Bibliothèques dynamiques (optionnel)
├── assets/
│   └── ...               # Ressources statiques
└── checksums.blake3       # Hash BLAKE3 de chaque fichier
```

La signature couvre l'ensemble du bundle (manifest + contenu). La vérification se fait via `crypto_server`, pas en userland direct.

---

## 6. Registre ExoOS

Le registre officiel est un index ExoFS signé :

```
https://registry.exoos.dev/
    index.exo              # Index signé (BLAKE3 + Ed25519)
    packages/
        exo-calendar/
            1.0.0.exo-bundle
            1.0.0.exo-bundle.sig
        ...
    compat/
        calendar/
            2.11.1.exo-bundle
        vim/
            9.1.0.exo-bundle
        curl/
            8.7.0.exo-bundle
```

**Miroir de compatibilité POSIX :** Pour les apps POSIX, `exo` peut se connecter à un miroir qui repackage les binaires Debian/Alpine dans le format `.exo-bundle` avec génération automatique du manifest de capabilities (analyse statique des dépendances et syscalls).

---

## 7. Installation dans ExoFS

Les apps installées ne vont **pas** dans un `/usr/bin` global. Elles sont des objets ExoFS dans un namespace dédié :

```
/sys/           # Objets kernel (capabilities, config noyau)
/srv/           # Serveurs Ring1 (vfs_server, crypto_server, ...)
/apps/          # Applications natives ExoOS
    exo-calendar/
        bin/exo-calendar      [x  r-x---- [✓sig] @3d8f]
        config.exo            [c  rw----- ·      @2b9d]
        data/                 [d  rwxl--- ·      @9f3c]
/compat/        # Applications POSIX (sandbox)
    calendar/
        bin/calendar          [x  r-x---- [✓sig] @8a2c]
        lib/libc.so           [b  r------ [✓sig] @1f7b]
        _sandbox/             [d  rwxl--- [enc]  @4e9d]  ← espace privé
/home/
    eric/
        ...
```

La sandbox POSIX voit un filesystem "virtuel" monté via musl-exo, avec les chemins POSIX attendus (`/bin/`, `/usr/`, `/lib/`) mappés vers les objets ExoFS de sa sandbox.

---

## 8. Désinstallation

```bash
exo remove exo-calendar
```

Processus :
1. Révocation de toutes les capabilities liées au paquet (`security_server`)
2. Suppression des objets ExoFS (blobs, dirs) — décrémentation refcount
3. Si refcount == 0 : déduplication ExoFS récupère l'espace (GC epoch)
4. Mise à jour de l'index
5. Entrée dans ExoLedger : `uninstall:exo-calendar  epoch:43  revoked_caps:[...]`

---

## 9. Commande `exo doctor`

Vérifie la santé du système et signale les problèmes :

```
$ exo doctor

ExoOS v0.2.0 — Diagnostic Système
══════════════════════════════════════

[✓]  Kernel stability          98.2%  (target: 98%)
[✓]  ExoPhoenix                active  kernel-A  ep:43
[✓]  security_init             active  all 8 components
[✓]  crypto_server             running  cap@4421
[✓]  vfs_server                running  ExoFS ep:43
[✓]  network_server            running  smoltcp  192.168.1.42/24
[✓]  device_server             running  3 PCI devices
[✓]  ExoLedger                 ep:43  4422 entries  ✓sig
[✓]  musl-exo                  build:2026-05-14  syscalls:127/241

[WARN]  ext4_server            not started  (no ext4 volume detected)
[WARN]  fat_server             not started  (no FAT volume detected)
[INFO]  exo-pkg registry       https://registry.exoos.dev  ✓  last-sync: 2h ago
[INFO]  Installed packages     3 native, 2 compat

Recommandations:
  - Aucun problème critique détecté
  - 114 syscalls POSIX non implémentés (non bloquant pour apps installées)
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-PKG.md*
