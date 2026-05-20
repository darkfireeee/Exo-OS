# INDEX — Corpus Documentaire ExoOS v0.2.0
## Direction Complète des Bibliothèques & Architecture

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Version :** 1.0 FINAL

---

## Structure du Corpus

Ce corpus remplace et invalide les roadmaps précédentes (v1 et v2 fournies par ChatGPT). Il constitue la **référence unique** pour le développement de la couche bibliothèques d'ExoOS v0.2.0.

---

## Documents — Ordre de Lecture Recommandé

### 1. Vision & Analyse

| Document | Rôle | Lire en premier si... |
|----------|------|----------------------|
| `VISION-V0.2.0.md` | Périmètre officiel de la v0.2.0 | Toujours — lu avant tout autre |
| `ANALYSE-CRITIQUE-ROADMAPS.md` | Pourquoi les roadmaps ChatGPT sont incorrectes | Tu arrives d'une session précédente |
| `DIRECTION-LIBS-GLOBAL.md` | Classification A/B/C de toutes les libs | Tu veux savoir quelle lib utiliser |
| `LIBS-REJECTION-LOG.md` | Rejets définitifs justifiés | Quelqu'un propose une lib rejetée |

### 2. Spécifications Techniques

| Document | Contenu |
|----------|---------|
| `SPEC-EXO-DISPLAY-PROTOCOL.md` | Format d'affichage natif ExoOS (pas de rwx) |
| `SPEC-EXO-PKG.md` | Gestionnaire de paquets `exo` + commande correcte |
| `SPEC-EXO-SECURITY-ACTIVATION.md` | Activation complète des 7 composants de sécurité |
| `SPEC-EXOPHOENIX-V0.2.md` | ExoPhoenix parfait — bascule A↔B < 500ms |
| `SPEC-EXO-CRATES.md` | exo-alloc · exo-net · exo-crypto · exo-fs · exo-runtime |
| `SPEC-EXO-LIBC.md` | musl-exo — 127 syscalls, sandbox POSIX |
| `SPEC-EXO-GRAPHICS.md` | winit + wgpu + iced + fb_server |
| `SPEC-EXO-OBSERVABILITY.md` | log + tracing + monitor_server |
| `SPEC-EXO-DRIVERS-V0.2.md` | virtio, NVMe, AHCI, e1000e, rtl8139 |

### 3. Planification & Validation

| Document | Contenu |
|----------|---------|
| `ROADMAP-IMPLEMENTATION-V0.2.md` | 8 phases, dépendances, ordre de développement |
| `MASTER-CHECKLIST-V0.2.md` | 143 critères de validation — document vivant |

---

## Décisions Clés à Retenir

### ✅ La bonne commande pour installer une app POSIX

```bash
# CORRECT pour ExoOS :
exo compat install calendar
exo compat install vim
exo compat install curl
exo compat install vlc   # s'installe, avertit pour Wayland

# INCORRECT (n'existe pas dans ExoOS) :
apt install calendar
```

### ✅ Le bon format d'affichage

```
# ExoOS natif — CORRECT :
d  rwxl---  ·         @9f3c  ep:0042  4 entries  --------  documents/
x  r-x----  [✓sig]    @3d8f  ep:0038  2.1 MiB    9e4a72f1  shell

# POSIX — INTERDIT dans les outils natifs ExoOS :
drwxr-xr-x  2 eric users 4096 mai 14  documents/
```

### ✅ Les libs définitivement rejetées

`linux-pam` · `shadow-rs` · `libsodium` · `libfuse` · `rtnetlink` · `systemd` · `launchd` · `zbus` · `relibc` · `async-std` · `tokio` (runtime) · `pkgcraft`

### ✅ Les priorités de développement absolues

1. `exo-alloc` → sans allocateur, rien ne compile
2. `musl-exo` core → sans fork/exec, pas de processus
3. `exo-crypto` → sans crypto, sécurité = stub
4. `exo-net` + smoltcp → réseau
5. `exo-fs` natif → ExoFS aux apps
6. `exo-pkg` → installeur
7. `exo-runtime` → async
8. `exo-graphics` → shell visuel

### ✅ La règle ExoPhoenix-Safety

Toute lib à état **doit** implémenter `PhoenixSafe` :
- `on_pre_switch()` : invalider/sauvegarder l'état avant bascule
- `on_post_switch()` : réinitialiser depuis les capabilities survivantes

### ✅ La règle DRV-ARCH-01

Zéro logique de driver en Ring0. Les ISR ne font que : acquitter + flag atomique + EOI. Toute la logique de protocole est en Ring1.

---

## Métriques de Succès v0.2.0

| Métrique | Cible |
|----------|-------|
| Stabilité kernel | ≥ 98% |
| ExoPhoenix recovery | < 500ms |
| Tests sécurité | 13/13 PASS |
| Syscalls POSIX couverts | ≥ 127 |
| `calendar` via `exo compat install` | Fonctionnel |
| `curl https://` via `exo compat install` | Fonctionnel |
| Tests unitaires total | 100% PASS |
| Checklist MASTER | 143/143 |

---

## Prochaine Version

**ExoOS v0.3.0** — Visual & Applications

- Compositeur Wayland natif ExoOS
- Accélération GPU (DRM/KMS + wgpu hardware backend)
- Serveur de notifications
- Applications GUI de base
- Gestionnaire de mises à jour avec UI
- `exo compat install vlc` → fonctionne réellement avec audio/vidéo

---

*claude-alpha — ExoOS v0.2.0 — INDEX.md*
