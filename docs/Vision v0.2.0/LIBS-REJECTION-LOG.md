# LIBS-REJECTION-LOG — Journal de Rejet des Bibliothèques Incompatibles
## ExoOS v0.2.0 — Décisions Définitives

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** DÉFINITIF — Ces décisions ne sont pas réversibles pour v0.2.x

---

## Préambule

Ce document enregistre chaque bibliothèque **rejetée** avec la justification technique complète. L'objectif est double :

1. Éviter que ces choix soient remis en question à chaque session de développement
2. Fournir une réponse claire à quiconque proposerait d'intégrer ces libs

Le rejet n'est pas une question de préférence — c'est une **incompatibilité architecturale** dans chaque cas.

---

## REJ-001 — `linux-pam`

**Raison du rejet : Modèle de sécurité fondamentalement incompatible**

PAM (Pluggable Authentication Modules) authentifie des *utilisateurs* identifiés par `uid/gid` en vérifiant des fichiers `/etc/shadow`, `/etc/passwd`. Il suppose l'existence d'un daemon (comme `sshd`, `sudo`) qui élève les privilèges via `setuid()`.

ExoOS n'a :
- Aucun utilisateur (`uid/gid` n'existent pas comme concept de sécurité)
- Aucun `/etc/shadow` ni `/etc/passwd` réels (seulement émulés pour compat POSIX)
- Aucune élévation de privilège via `setuid()` (no-op loggé dans ExoLedger)

Le modèle ExoOS : l'authentification se fait par **dérivation de capabilities**. Un processus prouve son identité en présentant un `CapToken` valide obtenu via le `crypto_server` (signature Ed25519). Il n'y a pas de "mot de passe système".

**Décision :** Rejet total. Aucun port possible. Remplacé par le système de capabilities ExoOS.

---

## REJ-002 — `shadow-rs`

**Raison du rejet : Objet métier inexistant dans ExoOS**

`shadow-rs` gère la base de données des mots de passe système (fichier `/etc/shadow`). Il n'y a pas d'utilisateurs dans ExoOS au sens Unix du terme. La notion de "shadow password" est architecturalement absente.

**Note :** La couche musl-exo retourne une entrée fictive pour `getpwuid(0)` (user "exouser") uniquement pour satisfaire les apps POSIX qui appellent `getpwuid()` au démarrage — cela ne justifie pas l'intégration de shadow-rs.

**Décision :** Rejet total. Non remplaçable — le concept n'a pas d'équivalent ExoOS.

---

## REJ-003 — `libsodium`

**Raison du rejet : Bibliothèque C en environnement no_std Rust**

`libsodium` est une bibliothèque C. L'intégrer dans ExoOS implique :

1. Une FFI C depuis Rust → risque de violations de mémoire (libsodium gère sa propre mémoire avec des patterns C : `sodium_malloc`, `sodium_free`)
2. `libsodium` utilise `malloc`/`free` de la libc — en environnement no_std, la libc n'est pas disponible avant que musl-exo soit initialisé
3. `libsodium` utilise `getrandom()` ou `/dev/urandom` pour son entropie — ces chemins doivent être câblés vers le `crypto_server` ExoOS, mais libsodium ne fournit pas de hook pour le faire proprement
4. La surface de vulnérabilité FFI C est incompatible avec les garanties de sécurité ExoOS

**Alternative directe :** RustCrypto (`rustcrypto-aeads`, `rustcrypto-hashes`, `ring`) offrent les mêmes primitives (ChaCha20-Poly1305, XSalsa20, Ed25519, X25519) en Rust pur no_std, sans FFI.

**Décision :** Rejet total. Remplacé par la stack RustCrypto + ring dans `crypto_server`.

---

## REJ-004 — `libfuse`

**Raison du rejet : Modèle de driver incompatible avec Ring1**

FUSE (Filesystem in Userspace) fonctionne via `/dev/fuse` — un device kernel Linux qui forward les opérations VFS vers un processus userland via des callbacks. Ce modèle suppose :

1. Un kernel Linux (ou compatible) avec le module FUSE chargé
2. Un device `/dev/fuse` accessible via `ioctl()`
3. Des callbacks dans un thread blocking (read sur `/dev/fuse`)

ExoOS n'a pas de `/dev/fuse`. Son équivalent fonctionnel est le serveur `vfs_server` en Ring1, qui reçoit les opérations VFS via IPC SpscRing — non via un device FUSE.

Un filesystem custom dans ExoOS s'écrit comme un serveur Ring1 avec l'interface IPC correcte, pas comme un processus FUSE.

**Décision :** Rejet total. Le modèle driver ExoOS (IPC Ring1) est supérieur à FUSE.

---

## REJ-005 — `rtnetlink`

**Raison du rejet : Protocole inexistant dans ExoOS**

rtnetlink est le protocole Linux de gestion des interfaces réseau (routes, adresses IP, interfaces) via des sockets Netlink (`AF_NETLINK`). ExoOS n'implémente pas Netlink.

La gestion réseau dans ExoOS se fait via IPC vers `network_server` (Ring1) :
- Configuration IP : `NetRequest::SetAddress { iface, addr, prefix }`
- Routes : `NetRequest::AddRoute { dest, gateway, metric }`
- DHCP : `dhcp4r` intégré dans `network_server`

Tout ce que `rtnetlink` fait peut être fait via l'IPC `network_server` ExoOS avec des latences inférieures (pas de socket AF_NETLINK overhead).

**Décision :** Rejet total. Remplacé par l'API IPC `network_server`.

---

## REJ-006 — `systemd-upstream`

**Raison du rejet : Monolithe incompatible sur tous les plans**

systemd est un système d'init et gestionnaire de services pour Linux. Il suppose :

| Ce que systemd suppose | Ce qu'ExoOS a |
|------------------------|--------------|
| D-Bus pour l'IPC | IPC SpscRing natif (incompatible D-Bus) |
| cgroups v2 pour l'isolation | Scheduler ExoOS + ExoKairos |
| `uid/gid` pour les services | CapTokens |
| `/proc`, `/sys` Linux | Émulation partielle pour compat |
| Journald pour les logs | ExoLedger + tracing |
| Netlink pour le réseau | IPC network_server |
| Mount namespaces Linux | VFS namespaces ExoOS |

ExoOS a son propre `init_server` (PID1 Ring1) qui gère le démarrage des serveurs Ring1 dans l'ordre canonique (12 étapes). Il est suffisant, minimal, et conçu pour ExoOS.

**Note :** La roadmap ChatGPT suggérait de s'en "inspirer". L'inspiration utile est documentée dans `init_server` — pas besoin de référencer systemd directement.

**Décision :** Rejet total. `init_server` ExoOS est l'équivalent natif.

---

## REJ-007 — `launchd-upstream`

**Raison du rejet : Écosystème macOS inapplicable**

launchd est le gestionnaire de services macOS/XNU. Il utilise des plists, des ports Mach, et des APIs spécifiques à XNU. Aucun de ces concepts n'existe dans ExoOS.

Il n'y a pas même d'inspiration valable ici — l'architecture de services ExoOS est plus proche d'un microkernel L4 que de macOS.

**Décision :** Rejet total. Aucune valeur pour ExoOS.

---

## REJ-008 — `zbus`

**Raison du rejet : IPC alternatif créant un doublon conflictuel**

`zbus` est une implémentation Rust de D-Bus, un protocole IPC basé sur des sockets UNIX et un daemon (`dbus-daemon`). Intégrer zbus dans ExoOS créerait :

1. **Un deuxième système IPC** : ExoOS a un IPC natif (SpscRing, 50M msgs/s, latence ~2µs). D-Bus ajoute un broker intermédiaire, des sérialisations inutiles, et des latences de l'ordre de la milliseconde.
2. **Un daemon supplémentaire** : `dbus-daemon` devrait tourner en Ring3 avec des capabilities larges — risque de sécurité.
3. **Un modèle de sécurité différent** : D-Bus utilise des politiques XML (`dbus-daemon-system.conf`) incompatibles avec les CapTokens ExoOS.
4. **Une dette de compatibilité** : toute app utilisant zbus deviendrait dépendante de `dbus-daemon` — une contrainte perpétuelle.

**Décision :** Rejet total. L'IPC ExoOS natif est strictement supérieur pour tous les cas d'usage.

---

## REJ-009 — `relibc-git-upstream`

**Raison du rejet : Redondance avec musl-exo**

`relibc` (Redox libc) est une alternative à musl pour les systèmes Rust. Elle est valide techniquement mais :

1. ExoOS choisit `musl` comme base pour `musl-exo` — musl est plus mature, plus couverture de syscalls, meilleure documentation
2. Maintenir deux forks de libc (musl-exo + relibc-exo) doublerait la charge de maintenance sans bénéfice
3. La compatibilité binaire serait divisée — les packages compilés pour l'un ne tourneraient pas sur l'autre

**Décision :** Rejet en tant que fork principal. `musl-exo` est le fork canonique. `relibc` peut rester dans vendors comme référence, non maintenu.

---

## REJ-010 — `async-std-upstream`

**Raison du rejet : Redondance avec exo-runtime**

`async-std` est un runtime asynchrone Rust avec une API similaire à `std`. ExoOS développe `exo-runtime` (basé sur le scheduler ExoOS). Maintenir deux runtimes async crée :

1. Une incohérence pour les développeurs (lequel utiliser ?)
2. Des dépendances croisées difficiles à gérer
3. `async-std` dépend de features `std` qui ne sont pas toutes disponibles en no_std

**Décision :** Rejet. `exo-runtime` est le runtime canonique. Les types `async-std::sync::*` peuvent être portés individuellement dans `exo-runtime` si nécessaire.

---

## REJ-011 — `pkgcraft-upstream`

**Raison du rejet : Hors périmètre v0.2.0, sujet à révision**

`pkgcraft` est un gestionnaire de paquets style Portage (Gentoo). Il est trop complexe pour v0.2.0 et suppose un écosystème d'ebuilds qui n'existe pas pour ExoOS.

ExoOS v0.2.0 utilise `exo-pkg` (gestionnaire natif) documenté dans `SPEC-EXO-PKG.md`.

**Décision :** Rejet pour v0.2.0. Réévaluation possible en v0.4.0+ si le registre ExoOS grandit suffisamment pour justifier un système Portage-like.

---

## REJ-012 — `tokio` (runtime complet)

**Raison du rejet partiel : Le runtime tokio ne peut pas tourner sur ExoOS tel quel**

Tokio **en tant que runtime** suppose :
- `epoll` (Linux) pour l'I/O asynchrone → ExoOS n'a pas epoll (uniquement IPC poll)
- `io_uring` optionnel → non implémenté en v0.2.0
- Thread pool interne → compatible en théorie, mais interdépendance complexe avec le scheduler ExoOS
- `tokio::time` → basé sur `timerfd` Linux → non disponible

**Ce qui EST utilisable de tokio :**

| Module tokio | Utilisable | Notes |
|---|---|---|
| `tokio::sync::*` | ✅ Oui | Mutex, RwLock, mpsc, oneshot, broadcast — purement Rust, no_std possible |
| `tokio::task::*` | ⚠️ Partiel | `JoinHandle`, `spawn_blocking` — à tester |
| `tokio::io` traits | ✅ Oui | `AsyncRead`, `AsyncWrite` comme traits — compatibles avec exo-runtime |
| `tokio::runtime` | ❌ Non | Incompatible epoll/io_uring |
| `tokio::net` | ❌ Non | Dépend du runtime |
| `tokio::fs` | ❌ Non | Dépend du runtime |
| `tokio::time` | ❌ Non | Dépend de timerfd |

**Décision :** Rejet du runtime tokio. Les crates `tokio::sync` et les traits `tokio::io` peuvent être réexportés depuis `exo-runtime` pour faciliter la compatibilité des crates tierces.

---

## Tableau Récapitulatif

| REJ | Bibliothèque | Catégorie de rejet | Alternatif ExoOS |
|-----|-------------|-------------------|-----------------|
| REJ-001 | linux-pam | Modèle sécurité incompatible | CapToken system |
| REJ-002 | shadow-rs | Concept inexistant | N/A (pas d'utilisateurs) |
| REJ-003 | libsodium | FFI C no_std interdit | RustCrypto + ring |
| REJ-004 | libfuse | Driver model incompatible | Serveur Ring1 IPC |
| REJ-005 | rtnetlink | Protocole inexistant | IPC network_server |
| REJ-006 | systemd | Monolithe tout incompatible | init_server ExoOS |
| REJ-007 | launchd | Écosystème macOS inapplicable | init_server ExoOS |
| REJ-008 | zbus/D-Bus | IPC concurrent conflictuel | IPC SpscRing natif |
| REJ-009 | relibc | Redondant avec musl-exo | musl-exo |
| REJ-010 | async-std | Redondant avec exo-runtime | exo-runtime |
| REJ-011 | pkgcraft | Hors périmètre v0.2.0 | exo-pkg |
| REJ-012 | tokio (runtime) | epoll/io_uring absent | exo-runtime |

---

*claude-alpha — ExoOS v0.2.0 — LIBS-REJECTION-LOG.md*
