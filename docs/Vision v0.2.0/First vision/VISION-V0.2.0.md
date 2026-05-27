# ExoOS v0.2.0 — Vision & Périmètre Officiel
## Document de Référence — Lu avant tout autre

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** FONDATEUR — ne pas modifier sans révision complète du corpus

---

## 1. Déclaration d'Intention

La v0.2.0 est la **release de maturité structurelle** d'ExoOS.

Elle n'ajoute pas de fonctionnalités visuelles. Elle ne cible pas les applications grand public. Elle construit **le socle irréversible** sur lequel toutes les versions futures s'appuieront : un noyau à ~98% de maturité, une sécurité entièrement activée, un système de fichiers dont les primitives uniques sont exploitées, et une première capacité réelle à exécuter des programmes POSIX via une couche de compatibilité propre.

À la fin de v0.2.0, ExoOS doit pouvoir répondre : **"oui"** à chacune de ces questions :

- Le noyau est-il stable à ~98% (mémoire, scheduler, IPC, process, FS, sécurité) ?
- ExoPhoenix bascule-t-il A↔B de manière reproductible et sans perte de capabilities ?
- La sécurité zero-trust, captoken, ExoShield, ExoCage, ExoKairos, ExoLedger est-elle **active en production** (pas juste compilée) ?
- Peut-on installer et exécuter un programme POSIX utile (ex: `calendar`) via `exo compat install` ?
- Le format d'affichage des données reflète-t-il le modèle ExoOS (capabilities, pas rwx) ?
- Les bibliothèques intégrées amplifient-elles les avantages du noyau au lieu de les masquer ?

---

## 2. Ce que v0.2.0 N'Est PAS

| Hors périmètre | Pourquoi | Version cible |
|---|---|---|
| Wayland / compositeur | Pas de serveur graphique | v0.3.0 |
| Applications GUI (VLC, browser) | Dépendent de Wayland | v0.3.0+ |
| Serveur de notifications | Dépend de Wayland/D-Bus | v0.3.0 |
| Gestionnaire de mises à jour GUI | Dépend de Wayland | v0.3.0 |
| D-Bus / zbus | Incompatible IPC ExoOS | Jamais |
| PAM / shadow / systemd | Incompatibles modèle capability | Jamais |
| Applications POSIX graphiques | Attendent Wayland | v0.3.0+ |
| `apt install X` | Format incorrect pour ExoOS | Jamais (`exo compat install X`) |

---

## 3. Les Cinq Piliers de v0.2.0

### Pilier 1 — Kernel à ~98%

**Objectif :** Tout sous-système est fonctionnel, stable, testé.

| Sous-système | État actuel | Cible v0.2.0 |
|---|---|---|
| Mémoire (buddy, SLUB, vmalloc, CoW, swap) | ~85% | 98% |
| Scheduler (CFS, RT, deadline, SMP, FPU) | ~80% | 98% |
| IPC (SpscRing, sync, SHM, RPC) | ~82% | 98% |
| Process (fork, exec, signal, wait, thread) | ~75% | 98% |
| FS (ExoFS + VFS bridge) | ~82% | 98% |
| Sécurité (capability, zero-trust, isolation) | ~70% | 98% |
| Drivers (PCI, virtio, DMA, IOMMU) | ~75% | 95% |
| ExoPhoenix (dual-kernel, SSR, resurrection) | ~90% | 100% |

**Définition de "98%" :** Tous les chemins critiques testés, zéro P0 ouvert, zéro deadlock connu, zéro memory leak sur les tests de stress (2h+).

### Pilier 2 — Sécurité Entièrement Active

**Objectif :** Pas un seul composant de sécurité en mode "stub" ou "bypass".

Le schéma d'activation complet :

```
Boot ExoOS
    │
    ├─[Phase 5]─► security_init() ───────────────────────────────────┐
    │                                                                  │
    │             ┌────────────────────────────────────────────────┐  │
    │             │         CHAÎNE DE SÉCURITÉ COMPLÈTE            │  │
    │             │                                                 │  │
    │             │  ExoSeal (inverted boot, hash chain)           │  │
    │             │      ↓                                         │  │
    │             │  ExoCage (CET hardware, shadow stack)          │  │
    │             │      ↓                                         │  │
    │             │  Zero Trust (label sur chaque IPC)             │  │
    │             │      ↓                                         │  │
    │             │  CapToken (chaque ressource = token)           │  │
    │             │      ↓                                         │  │
    │             │  ExoKairos (budget temporel inline)            │  │
    │             │      ↓                                         │  │
    │             │  ExoLedger (audit immutable de tous les accès) │  │
    │             │      ↓                                         │  │
    │             │  ExoShield (IOMMU statique NIC, isolation DMA) │  │
    │             │      ↓                                         │  │
    │             │  ExoNMI (watchdog NMI kernel integrity)        │  │
    │             └────────────────────────────────────────────────┘  │
    │                                                                  │
    └──────────────────────────────────────────────────────────────────┘
```

**Critère de validation :** Un processus non privilégié ne peut obtenir aucune capability sans passer par le chemin d'autorisation. Toute tentative de bypass est auditée dans ExoLedger. Testé par suite de tests d'intrusion automatisés.

### Pilier 3 — ExoPhoenix Parfait

**Objectif :** Bascule A↔B reproductible, zéro perte de capabilities survivantes, recovery en < 500ms.

Voir `SPEC-EXOPHOENIX-V0.2.md` pour la spécification complète.

### Pilier 4 — Exécution POSIX via `exo compat`

**Objectif :** Installer et exécuter un programme POSIX en mode texte (ex: `calendar`, `vim`, `curl`).

La commande correcte dans ExoOS n'est pas `apt install`. C'est :

```
$ exo compat install calendar
```

Ce qui se passe en interne :
1. `exo-pkg` résout le bundle depuis le registre ExoOS (ou un miroir POSIX-compat)
2. Le bundle est signé et vérifié par `crypto_server`
3. Les binaires sont injectés dans ExoFS comme objets `blob` avec type `executable`
4. Un **manifest de capabilities** est généré : quels syscalls, quels IPC, quels objets FS l'app peut toucher
5. `musl-exo` fournit la couche POSIX → IPC ExoOS
6. L'app s'exécute en Ring3, isolée dans sa sandbox de capabilities

**En théorie pour v0.2.0 :** `exo compat install vlc` (résoudra le bundle, installera les binaires) mais **ne s'affichera pas** tant que Wayland n'existe pas (v0.3.0).

### Pilier 5 — Format d'Affichage ExoOS Natif

**Objectif :** Toutes les sorties des outils système reflètent le modèle ExoOS. Aucun `rwx`, aucun `uid/gid`.

Voir `SPEC-EXO-DISPLAY-PROTOCOL.md` pour la spécification complète.

---

## 4. Architecture des Bibliothèques — Vue Macroscopique

```
╔══════════════════════════════════════════════════════════════════════╗
║                    RING 3 — APPLICATIONS & LIBS USERLAND           ║
║                                                                      ║
║  exo-alloc    exo-net     exo-crypto   exo-fs    exo-runtime        ║
║  (snmalloc)   (IPC cli)   (IPC cli)    (IPC cli) (async executor)   ║
║                                                                      ║
║  ┌──────────────────────────────────────────────────────────────┐   ║
║  │              musl-exo + exo-libc (POSIX compat)             │   ║
║  │         (émulation syscall → IPC, couche optionnelle)        │   ║
║  └──────────────────────────────────────────────────────────────┘   ║
║                                                                      ║
║  exo-pkg (gestionnaire de paquets)  exo-observability (tracing)    ║
╠══════════════════════════════════════════════════════════════════════╣
║                    RING 1 — SERVEURS SYSTÈME                        ║
║                                                                      ║
║  network_server  crypto_server  vfs_server  device_server           ║
║  (smoltcp,       (RustCrypto,   (ExoFS,     (block/PCI,             ║
║   hickory-dns,    ring,          fat_server, virtio-net/blk)        ║
║   dhcp4r)         rustls)        ext_server)                        ║
╠══════════════════════════════════════════════════════════════════════╣
║                    RING 0 — KERNEL                                  ║
║                                                                      ║
║  memory/  scheduler/  ipc/  security/  exophoenix/  fs/  drivers/  ║
╚══════════════════════════════════════════════════════════════════════╝
```

---

## 5. Ordre de Priorité Absolu

Si une tâche bloque une autre, l'ordre est :

```
1. exo-alloc         (sans allocateur, rien ne compile en userland)
2. musl-exo core     (sans fork/exec, pas de processus)
3. exo-crypto        (sans crypto, la sécurité est stub)
4. exo-net           (smoltcp dans network_server)
5. exo-fs natif      (primitives ExoFS aux apps)
6. exo-pkg           (installateur)
7. exo-runtime       (async executor)
8. exo-observability (logging/tracing)
9. exo-libc étendu   (POSIX ~80%)
10. fat_server        (compatibilité volumes FAT/ext4)
```

---

## 6. Documents de ce Corpus

| Document | Contenu |
|---|---|
| `VISION-V0.2.0.md` (ce fichier) | Vision globale et périmètre |
| `ANALYSE-CRITIQUE-ROADMAPS.md` | Pourquoi les roadmaps ChatGPT sont incorrectes |
| `DIRECTION-LIBS-GLOBAL.md` | Classification de toutes les libs (A/B/C) |
| `SPEC-EXO-PKG.md` | Gestionnaire de paquets `exo` |
| `SPEC-EXO-DISPLAY-PROTOCOL.md` | Format d'affichage, permissions sans rwx |
| `SPEC-EXO-ALLOC.md` | Allocateur mémoire userland |
| `SPEC-EXO-NET.md` | Pile réseau (smoltcp + IPC) |
| `SPEC-EXO-CRYPTO.md` | Cryptographie (RustCrypto + ring) |
| `SPEC-EXO-FS.md` | Interface ExoFS native + compat |
| `SPEC-EXO-RUNTIME.md` | Runtime asynchrone exo-rt |
| `SPEC-EXO-LIBC.md` | Couche POSIX musl-exo |
| `SPEC-EXO-SECURITY-ACTIVATION.md` | Activation complète sécurité |
| `SPEC-EXOPHOENIX-V0.2.md` | ExoPhoenix parfait |
| `LIBS-REJECTION-LOG.md` | Log de rejet définitif des libs incompatibles |

---

*claude-alpha — ExoOS v0.2.0 — VISION-V0.2.0.md*
