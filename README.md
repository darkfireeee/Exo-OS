# Exo-OS — Système d'exploitation intelligent et sécurisé

![Exo-OS Badge](https://img.shields.io/badge/Exo--OS-Intelligent%20%26%20S%C3%A9curis%C3%A9-blue?style=for-the-badge&logo=shield&logoColor=white)
![Version](https://img.shields.io/badge/version-0.1.0--dev-green?style=for-the-badge)
![License](https://img.shields.io/badge/license-MIT%2FApache%202.0-yellow?style=for-the-badge)
![Architecture](https://img.shields.io/badge/architecture-x86__64-red?style=for-the-badge)
![Bootloader](https://img.shields.io/badge/bootloader-Multiboot2%20%2B%20GRUB-orange?style=for-the-badge)

Salut 👋 et bienvenue sur Exo-OS !

Exo-OS est un système d'exploitation de nouvelle génération qui combine une architecture microkernel sécurisée avec une intégration profonde de l'intelligence artificielle pour offrir une expérience utilisateur moderne, robuste et sécurisée.

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


