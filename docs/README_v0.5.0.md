# 📘 README - Exo-OS v0.5.0

![Exo-OS](https://img.shields.io/badge/Exo--OS-v0.5.0-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Architecture](https://img.shields.io/badge/arch-x86__64-orange)
![Language](https://img.shields.io/badge/language-Rust-red)

**Exo-OS** - Système d'exploitation microkernel moderne écrit en Rust

## 🎯 Version actuelle : 0.5.0 "Shell Complete"

### Nouveautés v0.5.0

- ✅ **Shell interactif complet** avec support VFS
- ✅ **14 commandes** (built-in + manipulation fichiers)
- ✅ **Interface ANSI** colorée avec prompt
- ✅ **Kernel nettoyé** (modules redondants supprimés)
- ✅ **Syscalls complets** (40+ appels système Linux-compatibles)

## 🚀 Démarrage rapide

### Prérequis

```bash
# Rust nightly
rustup default nightly
rustup component add rust-src
rustup target add x86_64-unknown-none

# Build tools
sudo apt install nasm clang qemu-system-x86
```

### Compilation

```bash
# Compiler le kernel
make build

# Créer l'image ISO
make iso

# Lancer dans QEMU
make run
```

### Utiliser le shell

Une fois Exo-OS démarré :

```bash
# Afficher l'aide
help

# Naviguer
ls /
cd /home
pwd

# Manipuler fichiers
touch test.txt
write test.txt "Hello Exo-OS"
cat test.txt
rm test.txt

# Créer répertoires
mkdir mydir
ls /
rmdir mydir

# Quitter
exit
```

## 📁 Structure du projet

```
Exo-OS/
├── kernel/               # Microkernel Rust
│   ├── src/
│   │   ├── arch/        # Code spécifique x86_64
│   │   ├── fs/          # VFS + filesystems
│   │   ├── memory/      # Gestion mémoire
│   │   ├── scheduler/   # Ordonnanceur threads
│   │   ├── syscall/     # Handlers syscalls
│   │   └── ...
│   └── Cargo.toml
├── userland/            # Programmes utilisateur
│   ├── shell/          # Shell interactif (v0.5.0)
│   ├── init/           # Processus init
│   └── ...
├── docs/               # Documentation
│   ├── CHANGELOG_v0.5.0.md
│   ├── ARCHITECTURE_v0.4.0.md
│   └── ...
└── Makefile
```

## 🛠️ Fonctionnalités

### ✅ Implémenté

#### Kernel
- [x] Boot multiboot2 (GRUB)
- [x] GDT, IDT, TSS configurés
- [x] Interruptions PIC 8259
- [x] Timer PIT/HPET
- [x] Clavier PS/2 (QWERTY/AZERTY)
- [x] Pagination 4 niveaux
- [x] Allocateur physique (bitmap)
- [x] Allocateur virtuel (buddy + slab)
- [x] VFS complet (tmpfs)
- [x] Syscalls Linux x86_64 (40+ calls)
- [x] Threads kernel
- [x] Scheduler round-robin

#### Userland
- [x] Shell interactif Exo-Shell
- [x] Commandes built-in (help, exit, clear, echo, pwd, cd, version)
- [x] Commandes VFS (ls, cat, mkdir, rm, rmdir, touch, write)
- [x] Support ANSI colors
- [x] Édition de ligne (backspace, Ctrl+C, Ctrl+D)

### 🚧 En cours

- [ ] Tests QEMU complets
- [ ] Fork/Exec pour processus externes
- [ ] Hello World userspace (/bin/hello)
- [ ] Support SMP multi-core
- [ ] Drivers réseau de base

### 📋 Planifié (v0.6.0+)

- [ ] Shell avec pipes et redirections
- [ ] Système de fichiers ext2/ext4
- [ ] Support USB
- [ ] Interface graphique basique
- [ ] Intégration AI (assistant shell)

## 🎓 Architecture

Exo-OS suit une architecture microkernel :

```
┌─────────────────────────────────────┐
│         Applications                │
│  (shell, userspace programs)        │
├─────────────────────────────────────┤
│      Syscall Interface (40+)        │
├─────────────────────────────────────┤
│         Microkernel                 │
│  ┌──────────┬──────────┬─────────┐ │
│  │  Memory  │ Scheduler│   IPC   │ │
│  ├──────────┼──────────┼─────────┤ │
│  │   VFS    │ Syscalls │ Drivers │ │
│  └──────────┴──────────┴─────────┘ │
├─────────────────────────────────────┤
│    Hardware Abstraction (x86_64)    │
└─────────────────────────────────────┘
```

### Composants clés

**Kernel** :
- Scheduler : Round-robin avec threads kernel
- Memory : Pagination 4-level, allocateurs physique/virtuel
- VFS : Abstraction filesystem avec tmpfs
- Syscalls : Interface Linux-compatible

**Userland** :
- Shell : Interface interactive no_std
- Services : À venir (network, fs_service, etc.)

## 📝 Développement

### Commiter

```bash
# Format code
cargo fmt --all

# Vérifier
cargo clippy --all

# Tests
cargo test --all

# Compiler
make build

# Commit
git add .
git commit -m "feat: description"
```

### Conventions

- Utiliser `feat:` pour nouvelles fonctionnalités
- Utiliser `fix:` pour corrections bugs
- Utiliser `refactor:` pour réorganisation code
- Utiliser `docs:` pour documentation uniquement

## 🤝 Contribuer

Les contributions sont bienvenues ! Consultez [CONTRIBUTING.md](CONTRIBUTING.md) pour les guidelines.

### Domaines prioritaires

1. **Tests** : Validation QEMU, unit tests
2. **Drivers** : Réseau, USB, AHCI
3. **Userspace** : Utilitaires système, programmes
4. **Documentation** : Tutoriels, exemples, API docs

## 📖 Documentation

- [CHANGELOG v0.5.0](docs/CHANGELOG_v0.5.0.md)
- [Architecture v0.4.0](docs/ARCHITECTURE_v0.4.0.md)
- [Syscalls](docs/README_v0.4.0.md)
- [VFS Documentation](kernel/src/fs/vfs/mod.rs)
- [Shell Source](userland/shell/src/)

## 🧪 Tests

```bash
# Tests unitaires kernel
cd kernel && cargo test

# Tests intégration
cargo test --test integration_tests

# Tests QEMU (à venir)
make test-qemu
```

## 🐛 Bugs connus

- 194 warnings Rust à nettoyer (non bloquants)
- SMP désactivé temporairement (sera réactivé en v0.6.0)
- getdents64 parsing peut nécessiter ajustements

## 📜 Licence

MIT License - Voir [LICENSE](LICENSE)

## 👥 Auteurs

- **darkfireeee** - Développeur principal

## 🙏 Remerciements

- Communauté Rust
- Projets OS dev : Redox, Theseus, Tock
- Tutoriels : OSDev Wiki, Phil Opp's Blog

---

**Version** : 0.5.0  
**Dernière mise à jour** : 3 décembre 2025  
**Statut** : En développement actif 🚀
