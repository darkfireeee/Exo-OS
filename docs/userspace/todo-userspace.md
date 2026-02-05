
L'ordre suit une logique de dépendance : du matériel le plus bas niveau jusqu'à l'expérience utilisateur finale.

## 🟠 Niveau 1 : Abstraction Matérielle & Pilotes (HAL)
*Interface entre le noyau et le matériel, utilisant l'approche "Shim Layer" pour le code Linux existant.*

*   **Graphique (DRM/KMS)**
    *   [x] **DRM Compatibility Layer** (Wrapper Rust pour les drivers Linux).
    *   [x] Drivers Linux statiques (Intel i915, AMD amdgpu, VirtIO).
    *   [x] **GEM Memory Manager** (Graphics Execution Manager).
*   **Son (Audio)**
    *   [x] **PipeWire** (Adapté pour utiliser Fusion Rings au lieu des sockets Unix pour la latence).
    *   [x] **ALSA Compatibility Layer**.
*   **Entrées (Input)**
    *   [x] **libinput** (Gestionnaire d'entrées unifié).
    *   [x] **evdev** (Implémentation custom du protocol evdev dans le noyau).
*   **Stockage & Systèmes de Fichiers**
    *   [x] **FUSE** (Filesystem in Userspace).
    *   [x] Support natif : **Ext4, FAT32, tmpfs**.
    *   [x] Support avancé : **Btrfs/ZFS** (Pour snapshots/rollback).
    *   [x] **LUKS-like** (Chiffrement de disque).
*   **Périphériques**
    *   [x] **USB Stack** (libusb + wrappers noyau).
    *   [x] **Bluetooth** (BlueZ).
    *   [x] **WiFi** (wpa_supplicant + implémentation nl80211 via IPC).
    *   [x] **Firmware Loader** (linux-firmware).
    *   [x] **Thunderbolt** (bolt).
    *   [x] **Capteurs** (iio-sensor-proxy).
    *   [x] **Empreintes** (libfprint).
    *   [x] **NVMe** (Support avancé).

---

## 🟡 Niveau 2 : Services Système & POSIX-X
*La couche de compatibilité et les démons système.*

*   **Compatibilité**
    *   [x] **POSIX-X Layer** (Implémentation hybride des syscalls Linux : Fast Path, Hybrid Path, Legacy Path).
    *   [x] **musl libc** (Adaptée pour Exo-OS).
*   **Gestion des Services**
    *   [x] **Init System** (Custom Rust).
    *   [x] **Service Manager** (s6-rc + Wrapper Rust pour orchestration).
    *   [x] **Device Manager** (eudev).
    *   [x] **D-Bus** (dbus-broker + zbus).
*   **Utilitaires Système**
    *   [x] **Logging** (syslog-ng + Indexation custom).
    *   [x] **Time Sync** (chrony).
    *   [x] **Multi-user** (Linux-PAM).
    *   [x] **Hardware Abstraction** (eudev).

---

## 🟣 Niveau 3 : Graphisme & Affichage (Display Server)
*La pile graphique moderne.*

*   **Rendu 3D / API**
    *   [x] **Mesa3D** (Vulkan, OpenGL, Gallium drivers - Patchée pour Exo-OS).
    *   [x] **Vulkan Loader**.
*   **Compositor**
    *   [x] **Wayland Compositor** (Base : smithay ou cosmic-comp).
    *   [x] **XWayland** (Compatibilité applications X11 legacy).
*   **Gestionnaire d'affichage**
    *   [x] **greetd** (Greffon d'authentification agnostique).

---

## 🔴 Niveau 4 : Environnement Bureau (Desktop Environment)
*L'interface utilisateur complète (Le "Gap" critique identifié).*

*   **Shell & UI**
    *   [x] **Cosmic Desktop** (Recommandé : 100% Rust, System76).
        *   [x] Panel (Barre des tâches).
        *   [x] App Launcher.
        *   [x] System Tray.
        *   [x] Settings App (Paramètres système).
    *   *Alternative :* GNOME Shell (Fork) ou KDE Plasma.
*   **Accessibilité**
    *   [x] **AT-SPI** (Protocole d'accessibilité).
    *   [x] **Orca** (Lecteur d'écran).
*   **Utilitaires DE**
    *   [x] **Clipboard** (wl-clipboard).
    *   [x] **Notifications** (mako).
    *   [x] **Thèmes & Icônes**.

---

## 🟢 Niveau 5 : Réseau & Connectivité
*Connectivité filaire et sans fil.*

*   **Stack Réseau**
    *   [x] **TCP/IP Stack** (Userspace ou Kernel).
    *   [x] **Firewall** (nftables + firewalld).
    *   [x] **VPN** (WireGuard intégré).
*   **Clients & Services**
    *   [x] **Network Manager** (Frontend pour WiFi/Ethernet).
    *   [x] **OpenSSH** (Accès distant).
    *   [x] **Cloud Integration** (GNOME Online Accounts).

---

## ⚪ Niveau 6 : Gestion des Logiciels (Software Delivery)
*Comment on installe et met à jour.*

*   **Gestionnaire de Paquets**
    *   [x] **Solver** (libsolv - Résolution de dépendances).
    *   [x] **Atomic Updates** (OSTree - Mises à jour atomiques).
    *   [x] **CLI** (exo-pkg).
    *   [x] **GUI** (Adaptation de GNOME Software).
*   **Conteneurs**
    *   [x] **Runtime** (crun - OCI compatible).
    *   [x] **Isolation** (Namespaces PID, NET, MNT...).
    *   [x] **Contrôle Ressources** (Cgroups v2).
    *   [x] **Filesystem** (OverlayFS).

---

## 🟤 Niveau 7 : Applications Utilisateur (Suite de base)
*Les applications indispensables pour un OS grand public.*

*   **Internet**
    *   [x] **Navigateur Web** (Chromium ou Falkon (Qt léger)).
    *   [x] **Email Client** (Thunderbird ou Geary).
*   **Bureautique**
    *   [x] **Suite Office** (LibreOffice complet ou AbiWord + Gnumeric).
    *   [x] **PDF Viewer**.
*   **Multimédia**
    *   [x] **Lecteur Vidéo** (VLC ou mpv).
    *   [x] **Lecteur Audio**.
    *   [x] **Visionneuse d'images**.
*   **Utilitaires**
    *   [x] **Terminal Emulator** (cosmic-term, alacritty ou kitty).
    *   [x] **Gestionnaire de fichiers** (cosmic-files, Nautilus ou Dolphin).
    *   [x] **Éditeur de texte**.
    *   [x] **Archivage** (7zip, Peazip).

---

## ⚫ Niveau 8 : Expérience Système & Maintenance
*Installation, maintenance et outils.*

*   **Installation**
    *   [x] **Live ISO**.
    *   [x] **Installateur GUI** (Calamares - Configuré pour Limine/Exo-OS).
*   **Gestion de l'énergie (Portable)**
    *   [x] **Power Profiles** (power-profiles-daemon).
    *   [x] **TLP** (Optimisation batterie avancée).
    *   [x] **Suspend/Resume** (Gestion ACPI).
*   **Maintenance**
    *   [x] **Sauvegardes** (Timeshift pour snapshots système + Restic pour chiffré).
    *   [x] **Mises à jour Firmware** (fwupd).
    *   [x] **Impression** (CUPS).
*   **Sécurité Avancée**
    *   [x] **Post-Quantum Crypto** (Kyber, Dilithium).
    *   [x] **SELinux-like framework** (LSM hooks).
*   **Jeux (Optionnel)**
    *   [x] **Steam** (Runtime Valve).
    *   [x] **GameMode** (Feral Interactive).
    *   [x] **Lutris** (Gestionnaire de jeux open-source).

---

## 🪙 Niveau 9 : Développement & Documentation
*Outils pour les développeurs.*

*   **Toolchain**
    *   [x] **Compilateurs** (GCC, Clang/LLVM).
    *   [x] **Rust Toolchain** (rustc, cargo).
    *   [x] **Build Systems** (Make, CMake, Ninja, Meson).
    *   [x] **Débogueurs** (gdb, lldb).
    *   [x] **VCS** (git).
*   **Qualité & Surveillance**
    *   [x] **Telemetry/Crash Reporting** (Sentry self-hosted).
    *   [x] **Audit de sécurité** (OpenSCAP).
*   **Documentation**
    *   [x] **Système d'aide** (mdBook).
    *   [x] **Man Pages**.

---

## 📝 Résumé de l'Intégration

| Composant | Stratégie | Source |
|-----------|-----------|--------|
| **Kernel** | Développé from-scratch en Rust | Interne |
| **Drivers (GPU/WiFi)** | Shim Layer / FFI | Linux Kernel |
| **OS Services** | Adaptation IPC (Fusion Rings) | PipeWire, Wayland |
| **Système de fichiers** | Portage utilisateur | FUSE, Btrfs |
| **Compatibilité** | Couche d'émulation POSIX | POSIX-X, musl |
| **Desktop** | Intégration directe | Cosmic Desktop |
| **Apps** | Compilation croisée (Cross-compilation) | Chromium, LibreOffice, etc. |

Cette liste ordonnée couvre l'intégralité des prérequis techniques et logiciels pour réaliser **Exo-OS** en tant que système d'exploitation grand public complet et moderne.