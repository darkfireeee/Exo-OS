# INDEX — Corpus Documentaire ExoOS v0.2.0 — Strata
## Référence Unique du Développement

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Version :** 2.0 — Remplace INDEX.md v1.0

---

## Corpus Strata vs Corpus v0.2.0 Initial

Ce corpus **remplace et invalide** l'ensemble du corpus v0.2.0 initial (produit le 2026-05-14).
Les documents de l'ancien corpus marqués `[SUPERSÉDÉ]` ne sont plus des références.

**Fichiers supersédés :**
- `VISION-V0.2.0.md` → remplacé par `VISION-STRATA.md`
- `INDEX.md` → remplacé par ce fichier
- `ROADMAP-IMPLEMENTATION-V0.2.md` → remplacé par `ROADMAP-STRATA.md`
- `MASTER-CHECKLIST-V0.2-REV2.md` → remplacé par `MASTER-CHECKLIST-STRATA.md`
- `SPEC-EXO-SECURITY-ACTIVATION.md` → remplacé par `SPEC-EXOSHIELD-STRATA.md`
- `SPEC-EXO-DRIVERS-V0.2.md` → remplacé par `SPEC-DRIVERS-STRATA.md`
- `ROADMAP-PHASE-0-CORRIGEE.md` → intégré dans `ROADMAP-STRATA.md`

**Fichiers conservés sans modification :**
- Tous les CORR-75 à CORR-86 (corrections kernel — toujours valides)
- `SPEC-EXOPHOENIX-V0.2.md` (valide — renommé `SPEC-EXOPHOENIX-STRATA.md`)
- `SPEC-EXO-DISPLAY-PROTOCOL.md`
- `SPEC-EXO-CRATES.md`
- `SPEC-EXO-LIBC.md`
- `SPEC-EXO-PKG.md`
- `SPEC-EXO-OBSERVABILITY.md`
- `DIRECTION-LIBS-GLOBAL.md`
- `LIBS-REJECTION-LOG.md`
- `ANALYSE-CRITIQUE-ROADMAPS.md`
- `WORKFLOW-MULTI-AI.md`
- `TOOLS-AUDIT-EXOOS.md`
- `MASTER-CORRECTIONS-V0.2.md`
- `SECURITY-CAPABILITY-TABLE-V0.2.md`
- `BOOT_SEQUENCE_V0.2.md` (mis à jour dans `BOOT_SEQUENCE_STRATA.md`)

---

## Ordre de Lecture Recommandé

### 1. Vision & Orientation (lire en premier)

| Document | Rôle | Lire si... |
|---|---|---|
| `VISION-STRATA.md` | Périmètre officiel, 6 piliers, identité | **Toujours — avant tout** |
| `ANALYSE-CRITIQUE-ROADMAPS.md` | Erreurs des roadmaps précédentes | Arrivée d'une nouvelle session |
| `DIRECTION-LIBS-GLOBAL.md` | Classification A/B/C des libs | Choix de dépendance |
| `LIBS-REJECTION-LOG.md` | Rejets définitifs | Proposition d'une lib |

### 2. Spécifications Nouvelles ou Mises à Jour (Strata)

| Document | Contenu | Statut |
|---|---|---|
| `SPEC-EXOSHIELD-STRATA.md` | Serveur EDR complet Ring1 — 9 modules | **NOUVEAU** |
| `SPEC-DRIVERS-STRATA.md` | Tous les drivers : AHCI, NVMe, USB, audio | **NOUVEAU** |
| `SPEC-BOOTLOADER-GPT-STRATA.md` | UEFI natif, schéma GPT, partitions ExoFS | **NOUVEAU** |
| `SPEC-USB-TRANSFER-STRATA.md` | Pipeline USB → ExoFS, mount, audit | **NOUVEAU** |
| `SPEC-AUDIO-STRATA.md` | Chime boot, terminal bell, alerte shield | **NOUVEAU** |
| `BOOT_SEQUENCE_STRATA.md` | Séquence 9 phases + vagues Ring1 | Mis à jour |

### 3. Spécifications Conservées

| Document | Contenu |
|---|---|
| `SPEC-EXOPHOENIX-STRATA.md` | ExoPhoenix parfait — bascule < 500ms |
| `SPEC-EXO-DISPLAY-PROTOCOL.md` | Format affichage natif (capabilities, pas rwx) |
| `SPEC-EXO-PKG.md` | Gestionnaire `exo` |
| `SPEC-EXO-CRATES.md` | exo-alloc, exo-net, exo-crypto, exo-fs, exo-runtime |
| `SPEC-EXO-LIBC.md` | musl-exo — 127 syscalls |
| `SPEC-EXO-GRAPHICS.md` | **Hors périmètre Strata** — fb_server minimal + v0.3.0 |
| `SPEC-EXO-OBSERVABILITY.md` | monitor_server, tracing, `exo log` |

### 4. Planification & Validation

| Document | Contenu |
|---|---|
| `ROADMAP-STRATA.md` | 12 phases, dépendances, ordre |
| `MASTER-CHECKLIST-STRATA.md` | Critères de validation — document vivant |
| `MASTER-CORRECTIONS-V0.2.md` | Historique corrections CORR-01 à CORR-86 |

### 5. Corrections Kernel (CORR series)

| Document | Contenu |
|---|---|
| `CORR-75.md` | SSR bitmask 256-core |
| `CORR-76-à-CORR-80.md` | Physmap 2G, ELF base, IPC cap, exosh sans réseau |
| `CORR-81-à-CORR-86.md` | SSR 4K, boot sécurité reséquencé, VirtIO BAR |

---

## Décisions Clés à Retenir — Strata

### ✅ La bonne commande pour installer une app POSIX
```bash
exo compat install calendar    # CORRECT
exo compat install vim
exo compat install curl
apt install calendar           # INCORRECT — n'existe pas dans ExoOS
```

### ✅ Le bon format d'affichage ExoOS natif
```
# CORRECT — capabilities, pas rwx :
d  rwxl---  ·         @9f3c  ep:0042  4 entries  --------  documents/
x  r-x----  [✓sig]    @3d8f  ep:0038  2.1 MiB    9e4a72f1  shell

# INTERDIT dans les outils natifs ExoOS :
drwxr-xr-x  2 eric users 4096 mai 14  documents/
```

### ✅ Le schéma de partitions ExoOS
```
GPT : ESP(FAT32, 256MB) | ExoFS ROOT(4GB+) | ExoFS DATA(reste)
Point d'entrée UEFI : EFI/EXOOS/BOOTX64.EFI
```

### ✅ ExoShield est un serveur, pas un composant
ExoShield démarre en **Vague 5** (dernier serveur Ring1) pour voir tous les
autres serveurs déjà actifs. Il n'est jamais initialisé comme un simple module
kernel — c'est un processus Ring1 indépendant avec son propre cycle de vie.

### ✅ L'audio v0.2.0 a un périmètre précis
Strata = chime boot + terminal bell + alertes sécurité.
Lecture multimédia, mixer → v0.3.0.

### ✅ Libs définitivement rejetées
`linux-pam` · `shadow-rs` · `libsodium` · `libfuse` · `rtnetlink`
`systemd` · `launchd` · `zbus` · `relibc` · `async-std`
`tokio` (runtime) · `pkgcraft`

---

## Métriques de Succès Strata

| Métrique | Cible |
|---|---|
| Stabilité kernel | ≥ 98% |
| ExoPhoenix recovery | < 500ms |
| Tests sécurité ExoShield | 13/13 PASS |
| Syscalls POSIX | ≥ 127 |
| `calendar` via `exo compat` | Fonctionnel |
| `curl https://` via `exo compat` | Fonctionnel |
| Transfert clé USB → ExoFS | Fonctionnel |
| Boot chime | Joué |
| UEFI natif sans GRUB | Fonctionnel |
| AHCI ou NVMe | ≥ 1 opérationnel |
| MASTER-CHECKLIST-STRATA | 100% |

---

*claude-alpha — ExoOS v0.2.0 — Strata — INDEX-STRATA.md*
