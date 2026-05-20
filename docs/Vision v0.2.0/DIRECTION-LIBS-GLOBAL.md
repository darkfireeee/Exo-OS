# Direction Complète des Bibliothèques — ExoOS v0.2.0
## Document Maître

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Version :** 1.0 FINAL  
**Périmètre :** Toutes les libs dans `libs/vendors/` et les crates `exo-*`

---

## 1. Philosophie Directrice

ExoOS n'est pas Linux. Ses différences architecturales ne sont pas des contraintes à contourner — ce sont des **avantages à amplifier** via les bibliothèques. Toute décision d'adaptation doit répondre à la question :

> *"Est-ce que cette intégration rend les capacités uniques d'ExoOS plus accessibles et plus performantes, ou les cache-t-elle derrière une émulation Linux ?"*

Les cinq avantages uniques à préserver et amplifier :

| Avantage | Mécanisme noyau | Obligation pour les libs |
|----------|----------------|------------------------|
| **IPC ultra-rapide** | SpscRing 50M msgs/s, fastcall ASM | Les libs de communication doivent l'utiliser, pas socket() |
| **Sécurité par capability** | `security/capability/` | Chaque ressource est un token capability, jamais un fd anonyme |
| **ExoPhoenix resurrection** | dual-kernel A/B, SSR | Les libs à état doivent être resurrection-safe |
| **ExoFS content-addressed** | epochs, relations, blobs, dedup | Les libs de stockage exposent ces primitives, pas juste POSIX |
| **Ring0/Ring1 isolation** | DRV-ARCH-01 | Les libs sont classifiées par ring, pas par domaine fonctionnel |

---

## 2. Architecture Globale des Libs

```
┌──────────────────────────────────────────────────────────────────────┐
│                     RING 3 — APPLICATIONS                           │
│                                                                      │
│   exo-net   exo-crypto   exo-fs   exo-runtime   exo-graphics       │
│   (client IPC)                    (exo-rt)      (iced/wgpu/winit)  │
│                                                                      │
│   ╔═══════════════════════════════════════════════════════╗         │
│   ║            exo-libc / musl-exo (compat POSIX)        ║         │
│   ║       (couche d'émulation, non prioritaire)           ║         │
│   ╚═══════════════════════════════════════════════════════╝         │
├──────────────────────────────────────────────────────────────────────┤
│                     RING 1 — SERVEURS SYSTÈME                       │
│                                                                      │
│   network_server    crypto_server    vfs_server    device_server    │
│   (smoltcp core)    (RustCrypto+ring) (ExoFS)      (block/pci)     │
│                                                                      │
├──────────────────────────────────────────────────────────────────────┤
│                     RING 0 — NOYAU                                  │
│                                                                      │
│   memory/   scheduler/   security/   ipc/   exophoenix/            │
│   Allocateur hybride (SLUB+vmalloc)  SpscRing  ExoFS kernel path   │
└──────────────────────────────────────────────────────────────────────┘
```

### 2.1 Principe de Placement

- **Ring1** : toute lib qui accède au matériel ou gère des secrets (crypto keys, DMA, block I/O)
- **Ring3** : toute lib applicative (HTTP client, runtime async, GUI, FS client via capability)
- **Ring0** : zéro lib externe. Le noyau ne dépend que de ses propres crates internes.

---

## 3. Classification des Librairies

### 3.1 GROUPE A — Adoptées, intégration native ExoOS

Ces libs sont directement compatibles avec le modèle ExoOS et doivent être intégrées en priorité.

| Lib | Ring | Rôle | Crate ExoOS |
|-----|------|------|-------------|
| `smoltcp` | 1 | Pile TCP/IP dans `network_server` | `exo-net` (Ring1 backend) |
| `hickory-dns` | 1 | Résolution DNS dans `network_server` | `exo-net` |
| `dhcp4r` | 1 | DHCP client/serveur dans `network_server` | `exo-net` |
| `rustcrypto-aeads` | 1 | AES-GCM, ChaCha20-Poly1305 | `exo-crypto` |
| `rustcrypto-hashes` | 1 | SHA-2, SHA-3, BLAKE3 | `exo-crypto` |
| `rustcrypto-kdfs` | 1 | HKDF, PBKDF2 | `exo-crypto` |
| `rustcrypto-password-hashes` | 1 | Argon2id | `exo-crypto` |
| `rustcrypto-rsa` | 1 | RSA-PKCS1, RSA-OAEP | `exo-crypto` |
| `rustcrypto-elliptic-curves` | 1 | ECDSA P-256/P-384, ECDH | `exo-crypto` |
| `ring` | 1 | Primitives ASM optimisées (fallback vitesse) | `exo-crypto` (backend perf) |
| `rustls` | 1 + 3 | TLS 1.3 au-dessus d'`exo-net` | `exo-tls` |
| `rust-fatfs` | 1 | Volumes FAT12/16/32 | `fat_server` |
| `redoxfs` | 1 | Volumes journalisés Redox-style | `redoxfs_server` |
| `snmalloc-rs` | 3 | Allocateur userland principal | `exo-alloc` |
| `dlmalloc` | 3 | Allocateur fallback | `exo-alloc` |
| `log` | 3 | Façade de logging | `exo-observability` |
| `tracing` | 3 | Instrumentation structurée | `exo-observability` |
| `rayon` | 3 | Parallélisme data-parallel | `exo-runtime` |
| `winit` | 3 | Événements clavier/souris/fenêtre | `exo-graphics` |
| `wgpu` | 3 | Rendu GPU abstrait | `exo-graphics` |
| `iced` | 3 | Framework GUI déclaratif | `exo-graphics` |

### 3.2 GROUPE B — Adoptées, adaptation requise (mode POSIX uniquement)

Ces libs nécessitent un portage non trivial mais restent viables pour la couche de compatibilité.

| Lib | Ring | Adaptation nécessaire | Délai cible |
|-----|------|-----------------------|-------------|
| `musl-exo` (fork musl) | 3 | Rediriger syscalls vers ExoOS IPC | v0.2.0 |
| `jemallocator` | 3 | Hooks mmap → exo_mmap uniquement | v0.2.0 |
| `libudev-rs` | 3 | Remplacer Netlink par IPC `device_server` | v0.3.0 |
| `tokio` (sync + io seuls) | 3 | Retirer runtime, garder types sync | v0.3.0 |
| `hyper` | 3 | Au-dessus d'exo-net + exo-rt | v0.3.0 |
| `axum` | 3 | Dépend de hyper + exo-rt | v0.3.0 |
| `cargo-chef` | build | Aucune — utilisé en toolchain | Immédiat |
| `ext4-rs` | 1 | Bridge C limité, évaluer alternatives pures Rust | v0.3.0 |

### 3.3 GROUPE C — Rejetées définitivement

Ces libs sont **incompatibles** avec l'architecture ExoOS et **ne doivent pas** être portées.

| Lib | Motif de rejet |
|-----|----------------|
| `linux-pam` | Authentification par UID/GID — ExoOS utilise les capabilities |
| `shadow-rs` | Gestion /etc/shadow — il n'y a pas d'utilisateurs dans ExoOS |
| `libsodium` | Bibliothèque C — FFI en no_std = undefined behavior potentiel |
| `libfuse` | Bibliothèque C — callbacks FUSE incompatibles avec IPC Ring1 |
| `rtnetlink` | Protocole Netlink Linux — inexistant dans ExoOS |
| `systemd-upstream` | Monolithique, gestion d'utilisateurs, D-Bus — tout est incompatible |
| `launchd-upstream` | Dépend de l'écosystème macOS/launchd — inapplicable |
| `zbus` | D-Bus IPC — doublon conflictuel avec l'IPC natif ExoOS |
| `relibc-git-upstream` | Redondant avec musl-exo (choisir un seul fork POSIX) |
| `pkgcraft-upstream` | Gestionnaire de paquets style Portage — post v0.2.0 |
| `async-std-upstream` | Redondant avec exo-rt — choisir un seul runtime async |

---

## 4. Les Crates ExoOS Canoniques

### 4.1 `exo-alloc` — Gestion Mémoire Userland

**Responsabilité :** Allouer et libérer de la mémoire en Ring3, en s'appuyant sur `mmap`/`munmap` ExoOS.

**Backends :**
- Primaire : `snmalloc-rs` (pools par thread, résistant aux use-after-free)
- Fallback : `dlmalloc` (architectures non supportées par snmalloc)
- Interdit : `jemallocator` dans Ring1 (risque de heap corruption cross-process)

**Connexion noyau :** `SYS_MMAP`, `SYS_MUNMAP`, `SYS_MPROTECT` — jamais `brk`/`sbrk` (non implémenté)

**Spec détaillée :** `SPEC-EXO-ALLOC.md`

### 4.2 `exo-net` — Pile Réseau

**Responsabilité :** Fournir des abstractions réseau capability-based aux applications Ring3. La pile smoltcp réside dans `network_server` (Ring1).

**Architecture :**
```
Ring3: exo-net (client IPC) ←→ IPC SpscRing ←→ Ring1: network_server (smoltcp)
```

**Spec détaillée :** `SPEC-EXO-NET.md`

### 4.3 `exo-crypto` — Cryptographie

**Responsabilité :** Exposer les primitives cryptographiques du `crypto_server` via des types capability-safe.

**Règle absolue :** Les clés privées ne sortent jamais du `crypto_server`. Les opérations voyagent en IPC, pas les secrets.

**Spec détaillée :** `SPEC-EXO-CRYPTO.md`

### 4.4 `exo-fs` — Interface ExoFS

**Responsabilité :** Exposer les primitives natives ExoFS (blobs, relations, epochs, snapshots) aux applications, avec une couche POSIX optionnelle par-dessus.

**Spec détaillée :** `SPEC-EXO-FS.md`

### 4.5 `exo-runtime` — Runtime Asynchrone

**Responsabilité :** Fournir un exécuteur asynchrone no_std basé sur le scheduler ExoOS, capable de piloter des futures sans dépendre de tokio.

**Spec détaillée :** `SPEC-EXO-RUNTIME.md`

### 4.6 `exo-graphics` — Pile Graphique

**Responsabilité :** Intégrer `winit` (événements), `wgpu` (GPU) et `iced` (GUI) dans le modèle ExoOS.

**Spec détaillée :** `SPEC-EXO-GRAPHICS.md`

### 4.7 `exo-libc` / `musl-exo` — Compatibilité POSIX

**Responsabilité :** Permettre l'exécution de binaires POSIX sur ExoOS via émulation syscall → IPC.

**Spec détaillée :** `SPEC-EXO-LIBC.md`

### 4.8 `exo-observability` — Observabilité

**Responsabilité :** Unifier `log` et `tracing` pour envoyer vers le `monitor_server` ExoOS.

---

## 5. ExoPhoenix-Safety : Règles Obligatoires

Toutes les libs à état persistant doivent implémenter le trait `PhoenixSafe` :

```rust
/// Trait obligatoire pour toute lib conservant un état inter-IPC.
pub trait PhoenixSafe {
    /// Appelé par ExoPhoenix avant toute bascule A→B.
    /// La lib doit invalider ou serialiser son état.
    fn on_pre_switch(&self) -> Result<(), PhoenixError>;
    
    /// Appelé par ExoPhoenix après la bascule, sur le nouveau kernel.
    /// La lib doit réinitialiser son état depuis les capabilities survivantes.
    fn on_post_switch(&self) -> Result<(), PhoenixError>;
    
    /// Retourne vrai si la lib peut survivre à une bascule sans action.
    fn is_stateless(&self) -> bool { false }
}
```

**Libs stateless (safe sans action) :**
- `exo-alloc` : les arènes snmalloc sont recréées depuis zéro après résurrection
- `rustcrypto-*` : bibliothèques pures sans état global
- `tracing`/`log` : état trivial (subscribers)

**Libs à état qui nécessitent `on_pre_switch` :**
- `exo-net` : les sockets TCP actives doivent être invalidées (les numéros de séquence TCP ne survivent pas)
- `exo-crypto` : les clés en cache local doivent être évincées (les capabilities survivent, les données locales non)
- `exo-graphics` (wgpu) : les ressources GPU doivent être libérées et recréées

---

## 6. Interconnexions IPC Canoniques

```
exo-net     ──[SpscRing]──► network_server (Ring1)
exo-crypto  ──[SpscRing]──► crypto_server  (Ring1)
exo-fs      ──[SpscRing]──► vfs_server     (Ring1)
exo-device  ──[SpscRing]──► device_server  (Ring1)
exo-graphics──[SHM+IPC]──► fb_server      (Ring1)
exo-libc    ──[Syscall] ──► kernel         (Ring0)
```

Toute communication entre libs en Ring3 se fait via :
1. **Mémoire partagée** (`SYS_MMAP` + capability de mapping) pour les gros volumes
2. **SpscRing** pour les messages de contrôle (< quelques Ko)
3. **Jamais** via des signaux POSIX, des pipes, ou D-Bus

---

## 7. Ordre de Développement v0.2.0

### Phase 1 — Fondations (bloquant pour tout le reste)
1. `exo-alloc` : snmalloc + mmap hooks (sans snmalloc, pas d'allocation dans les crates suivantes)
2. `musl-exo` : syscalls fork/exec/signal minimum (sans ça, pas de processus userland)
3. `generic-rt` : TLS + TCB access (infrastructure runtime)

### Phase 2 — Services Critiques
4. `exo-crypto` : RustCrypto + ring dans `crypto_server`
5. `exo-net` : smoltcp + dhcp4r + hickory-dns dans `network_server`
6. `exo-fs` : primitives ExoFS natives + bridge FAT/ext4

### Phase 3 — Expérience Utilisateur
7. `exo-runtime` : exo-rt async executor
8. `exo-graphics` : winit + wgpu + iced
9. `exo-observability` : tracing → monitor_server

### Phase 4 — Compatibilité Applicative (v0.3.0)
10. `exo-libc` complet : POSIX ~80% via musl-exo
11. `hyper` + `axum` au-dessus d'exo-net + exo-rt
12. `libudev-rs` porté via device_server IPC

---

## 8. Métriques de Succès

À la fin de v0.2.0, les métriques suivantes doivent être atteintes :

| Métrique | Cible |
|----------|-------|
| Latence IPC exo-net → network_server | < 10 µs (via SpscRing) |
| Débit TCP smoltcp (loopback) | > 1 Gbps |
| Temps d'allocation snmalloc (16B) | < 50 ns |
| Primitives POSIX disponibles dans musl-exo | > 80 syscalls |
| Primitives crypto disponibles dans exo-crypto | AES-GCM, ChaCha20, SHA-2/3, Argon2id, RSA, ECDSA |
| Résolution DNS via hickory-dns | < 100 ms (réseau local) |
| Rendu frame iced (720p) | > 60 FPS sur hardware supporté |
| Tests de régression libs | 100% pass avant merge |

---

*claude-alpha — ExoOS Library Direction — DIRECTION-LIBS-GLOBAL.md*
