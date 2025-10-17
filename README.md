# Exo-OS — Système d'exploitation intelligent et sécurisé

![Exo-OS Badge](https://img.shields.io/badge/Exo--OS-Intelligent%20%26%20S%C3%A9curis%C3%A9-blue?style=for-the-badge&logo=shield&logoColor=white)
![Version](https://img.shields.io/badge/version-1.2.0--dev-green?style=for-the-badge)
![License](https://img.shields.io/badge/license-MIT%2FApache%202.0-yellow?style=for-the-badge)
![Architecture](https://img.shields.io/badge/architecture-Microkernel-red?style=for-the-badge)
![Phase](https://img.shields.io/badge/phase-10%20IPC%20Complete-success?style=for-the-badge)

Salut 👋 et bienvenue sur Exo-OS !

Exo-OS est un système d'exploitation de nouvelle génération qui combine une architecture microkernel sécurisée avec une intégration profonde de l'intelligence artificielle pour offrir une expérience utilisateur moderne, robuste et sécurisée.

## 🚀 État Actuel du Développement

**Version** : 1.2.0-dev  
**Phase** : Phase 10 - IPC Message-Passing ✅ **COMPLÉTÉE**  
**Date** : 4 octobre 2025

### ✅ Phases Complétées

- [x] **Phase 1-7** : Foundation (GDT, IDT, Paging, Frame Allocator, Serial, VGA)
- [x] **Phase 8** : Heap Allocator (1MB @ 0x08000000, Vec/Box/String support)
- [x] **Phase 9** : Syscalls & User/Kernel Transition (SYSCALL/SYSRET, 6 syscalls)
- [x] **Phase 10** : IPC Message-Passing (4 IPC syscalls, 4 default channels, FIFO queues)

### 🔄 Phase Actuelle : Transition vers Phase 11

**Prochaine Phase** : Scheduler Multi-Agent (~4-5h)

**Objectifs Phase 11** :
- [ ] Task/Agent structure avec context save/restore
- [ ] Round-robin ou CFS scheduler
- [ ] Context switching entre agents
- [ ] IPC + Scheduler integration (blocked agents)
- [ ] User space setup (user code mapping)
- [ ] Multi-agent tests

### 📊 Métriques Actuelles

- **Kernel size** : 3.87 MB
- **Boot score** : 6/8 tests passing
- **IPC tests** : 5/5 passing ✅
- **Syscalls** : 8 fonctionnels (4 base + 4 IPC)
- **Default channels** : 4 (kernel, debug, broadcast, log)
- **Max channels** : 32 simultaneous
- **Max message size** : 64 
### 📚 Documentation Récente

- [Phase 10 Rapport Final](docs/PHASE_10_RAPPORT_FINAL.md) - Rapport complet 70+ pages
- [Phase 10 Quick Reference](docs/PHASE_10_QUICK_REFERENCE.md) - API reference rapide
- [Changelog](CHANGELOG.md) - Historique détaillé des changements

---

## 🌟 Caractéristiques principales

### 🧠 Intelligence artificielle intégrée

- **Système d'agents IA optimisé** avec plusieurs agents spécialisés
- **AI-Core** : orchestrateur avec clés éphémères post-quantiques
- **AI-Res** : gestion des ressources avec algorithme Eco++ inspiré de big.LITTLE
- **AI-User** : interface adaptative (PEG hybride + moteur d'intention SLM)
- **AI-Sec** : sécurité proactive (ex.: libFuzzer)
- **AI-Learn** : apprentissage via modèles fédérés et cryptographie homomorphe
- **Embedded AI Assistant** : commandes vocales/textuelles pour le contrôle système

### 🔒 Matrice de sécurité

| Couche      | Technologie               | Protection                |
|-------------|--------------------------:|:-------------------------:|
| Matériel    | TPM 2.0 + HSM             | Attestation au démarrage  |
| Mémoire     | ASLR + Marquage mémoire   | Protection anti-exploit   |
| Données     | Chiffrement (XChaCha20)   | Confidentialité           |
| Réseau      | WireGuard intégré         | Tunnel sécurisé           |
| IA          | Sandboxing WebAssembly    | Isolation des agents      |

### ⚡ Performance optimisée

- Équilibrage de charge prédictif
- Compression mémoire (Zstd) pour la RAM inactive
- Gestion dynamique des cœurs
- Underclocking dynamique et ordonnancement spécialisé

### 🛡️ Sécurité avancée

- Architecture Zero Trust
- Attestation matérielle TPM/HSM
- Chiffrement de bout en bout
- Analyse comportementale IA pour détection proactive
- Sécurité par capacités et cryptographie post-quantique

### 🔄 Écosystème auto‑optimisé

- Composants modulaires remplaçables à chaud
- Orchestration IA locale sans dépendance cloud
- Interface adaptative (du terminal aux conversations naturelles)
- Apprentissage continu

## 🏗️ Architecture

Exo-OS adopte une architecture hybride combinant microkernel et modules utilisateur :

### Espace noyau (Kernel space)

Le noyau minimal contient les primitives essentielles : gestion mémoire, ordonnancement, pilotes bas niveau, primitives de sécurité, support TPM/HSM et runtime WebAssembly.

### Espace utilisateur (Userland)

La plupart des services (IA, pilotes avancés, applications, bibliothèques) s'exécutent en userland pour favoriser sécurité et résilience.

## 🚀 Démarrage rapide

### Prérequis logiciels

- **Rust 1.70+** (avec rustfmt et clippy)
- **Linker** : Visual Studio Build Tools (Windows) ou GCC (Linux/macOS)
- **LLVM 15+**
- **NASM** (pour l'assemblage x86)
- **QEMU** (pour l'émulation)
- **Git**

### Matériel recommandé

- Processeur **x86_64** ou **ARM64** (VT-x/AMD-V conseillé)
- **8 GB RAM** recommandé
- **20 GB d'espace disque**
- **TPM 2.0** (optionnel)
- **HSM** (optionnel)

### Installation Windows

```powershell
# 1. Vérifier l'environnement
.\scripts\check_env.ps1

# 2. Installer Visual Studio Build Tools (si nécessaire)
# Télécharger : https://aka.ms/vs/17/release/vs_BuildTools.exe
# Sélectionner : "Développement Desktop en C++"

# 3. Installer les targets Rust
rustup target add aarch64-unknown-none
rustup target add x86_64-unknown-none

# 4. Compiler le kernel
.\scripts\build_windows.ps1 -Release

# Voir docs/QUICK_START_WINDOWS.md pour plus de détails
```

### Installation Linux/macOS

```bash
# Cloner le dépôt
git clone https://github.com/exo-os/Exo-OS.git
cd Exo-OS

# Installer les targets Rust
rustup target add aarch64-unknown-none
rustup target add x86_64-unknown-none

# Configurer le TPM (si disponible)
./scripts/tpm_setup.sh

# Configurer les modules HSM (si disponibles)
./scripts/hsm_setup.sh

# Configurer la cryptographie post-quantique
./scripts/post_quantum_setup.sh

# Bootstrap du système
./scripts/bootstrap.sh -c configs/hardware/x86_64.toml

# Compiler le noyau
cd kernel
cargo build --target x86_64-unknown-none --release
cargo build --target aarch64-unknown-none --release

# Générer l'image système
./build/image.sh
```

---

Pour les contributeurs : voir `CONTRIBUTING.md` et les scripts sous `scripts/` pour les étapes d'environnement et de build.


