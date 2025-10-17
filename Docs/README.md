# 📚 Documentation Exo-OS

Bienvenue dans la documentation complète du projet Exo-OS !

---

## 🚀 Pour Démarrer

### Documentation Essentielle

| Document | Description | Pour Qui |
|----------|-------------|----------|
| **[QUICKSTART.md](QUICKSTART.md)** | Guide de démarrage rapide | 🏃 Développeurs débutants |
| **[TESTING.md](TESTING.md)** | Guide complet de test et validation | 🧪 Testeurs |
| **[ROADMAP.md](ROADMAP.md)** | Plan de développement et optimisation | 🗺️ Contributeurs |
| **[BUILD_REPORT.txt](BUILD_REPORT.txt)** | Rapport de compilation actuel | 📊 État du projet |

---

## 📖 Documentation Technique

### Architecture et Composants

| Document | Sujet | Détails |
|----------|-------|---------|
| **[readme_kernel.txt](readme_kernel.txt)** | Structure du kernel | Point d'entrée, modules principaux |
| **[readme_x86_64_et_c_compact.md](readme_x86_64_et_c_compact.md)** | Architecture x86_64 | GDT, IDT, Interrupts, Code C |
| **[readme_memory_and_scheduler.md](readme_memory_and_scheduler.md)** | Mémoire et Ordonnancement | Frame allocator, Scheduler, Threads |
| **[readme_syscall_et_drivers.md](readme_syscall_et_drivers.md)** | Syscalls et Drivers | Interface système, Pilotes |

---

## 🎯 Par Objectif

### Je veux...

#### 🔨 Compiler le Kernel

→ **[QUICKSTART.md](QUICKSTART.md)** - Section "Compilation Manuelle"

```powershell
cd kernel
cargo +nightly build --target ../x86_64-unknown-none.json -Z build-std=core,alloc,compiler_builtins
```

---

#### 🧪 Tester le Kernel

→ **[TESTING.md](TESTING.md)** - Guide complet avec 3 méthodes

**Méthode Recommandée** (bootimage):
```powershell
cargo install bootimage
rustup component add llvm-tools-preview
cd kernel
cargo bootimage --run
```

---

#### 🏗️ Comprendre l'Architecture

→ **[readme_x86_64_et_c_compact.md](readme_x86_64_et_c_compact.md)**

Couvre:
- Structure x86_64 (GDT, IDT, Interrupts)
- Intégration C/Rust
- Port série et PCI

---

#### 🧠 Comprendre la Mémoire

→ **[readme_memory_and_scheduler.md](readme_memory_and_scheduler.md)**

Couvre:
- Frame allocator (allocation physique)
- Page tables (mémoire virtuelle)
- Heap allocator (tas kernel)
- Scheduler (threads, context switching)

---

#### 🔌 Comprendre les Syscalls et Drivers

→ **[readme_syscall_et_drivers.md](readme_syscall_et_drivers.md)**

Couvre:
- Interface d'appels système
- Dispatch des syscalls
- Architecture des pilotes
- Block devices

---

#### 🚀 Optimiser les Performances

→ **[ROADMAP.md](ROADMAP.md)** - Section "Phase 3: OPTIMISATION"

Objectifs de performance:
- IPC < 500ns
- Context Switch < 1µs
- Syscalls > 5M/sec
- Boot < 500ms

---

#### 🐛 Débugger un Problème

→ **[TESTING.md](TESTING.md)** - Section "Debugging"

Outils:
- Serial output (QEMU)
- GDB remote debugging
- Problèmes courants et solutions

---

## 📊 État du Projet

### Statistiques Actuelles

```
✅ Fichiers Rust: 21 (~66 KB)
✅ Fichiers C: 3 (~5 KB)
✅ Compilation: SUCCESS (0 erreurs, 42 warnings)
✅ Tests: Framework prêt
```

### Composants Implémentés

| Composant | État | Progression |
|-----------|------|-------------|
| **Architecture x86_64** | ✅ Fonctionnel | 90% |
| **GDT/IDT** | ✅ Configuré | 100% |
| **Interrupts** | ✅ Handlers définis | 80% |
| **Scheduler** | ✅ Implémenté | 70% |
| **IPC** | ✅ Channels lock-free | 80% |
| **Memory** | ⚠️ Stubs | 30% |
| **Syscall** | ⚠️ Stubs | 20% |
| **Drivers** | ⚠️ Stubs | 20% |

---

## 🗂️ Structure de la Documentation

```
Docs/
├── README.md                          ← Vous êtes ici
│
├── 🚀 DÉMARRAGE
│   ├── QUICKSTART.md                  Guide rapide
│   ├── TESTING.md                     Tests et validation
│   └── ROADMAP.md                     Plan de développement
│
├── 📖 TECHNIQUE
│   ├── readme_kernel.txt              Structure kernel
│   ├── readme_x86_64_et_c_compact.md  Architecture
│   ├── readme_memory_and_scheduler.md Mémoire et threads
│   └── readme_syscall_et_drivers.md   Syscalls et pilotes
│
└── 📊 RAPPORTS
    └── BUILD_REPORT.txt               État de compilation
```

---

## 🔗 Liens Utiles

### Documentation Externe

- **[OSDev Wiki](https://wiki.osdev.org/)** - Référence pour le développement OS
- **[Rust OSDev](https://os.phil-opp.com/)** - Blog sur Rust OS development
- **[Intel Manual](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)** - Manuel x86_64
- **[AMD64 ABI](https://refspecs.linuxbase.org/elf/x86_64-abi-0.99.pdf)** - Calling convention

### Outils Requis

| Outil | Usage | Installation |
|-------|-------|--------------|
| **Rust Nightly** | Compilation | `rustup default nightly` |
| **bootimage** | Images bootables | `cargo install bootimage` |
| **llvm-tools** | Outils LLVM | `rustup component add llvm-tools-preview` |
| **QEMU** | Test/émulation | `choco install qemu` ou [qemu.org](https://qemu.org) |
| **GDB** (optionnel) | Debugging | `choco install gdb` |

---

## 🎓 Apprentissage Progressif

### Parcours Débutant

1. **Jour 1**: Lire [QUICKSTART.md](QUICKSTART.md) → Compiler le kernel
2. **Jour 2**: Lire [readme_kernel.txt](readme_kernel.txt) → Comprendre la structure
3. **Jour 3**: Lire [TESTING.md](TESTING.md) → Tester avec QEMU
4. **Jour 4**: Lire [readme_x86_64_et_c_compact.md](readme_x86_64_et_c_compact.md) → Architecture
5. **Jour 5**: Modifier du code → Implémenter une feature simple

### Parcours Contributeur

1. Lire toute la documentation technique
2. Examiner le code source
3. Lire [ROADMAP.md](ROADMAP.md) pour les priorités
4. Choisir une tâche dans la roadmap
5. Implémenter + tester + documenter

### Parcours Optimisation

1. Établir baseline (voir [ROADMAP.md](ROADMAP.md) Phase 2)
2. Identifier les goulots d'étranglement
3. Implémenter optimisations ciblées
4. Benchmarker et valider
5. Documenter les résultats

---

## 🤝 Contribuer

### Améliorer la Documentation

La documentation peut toujours être améliorée ! Pour contribuer :

1. **Identifier** un point non clair ou manquant
2. **Éditer** le fichier markdown correspondant
3. **Tester** que vos instructions fonctionnent
4. **Commiter** avec un message clair

### Ajouter de la Documentation

Format suggéré pour de nouveaux documents :

```markdown
# Titre du Document

## Introduction
Brève description (1-2 phrases)

## Contexte
Pourquoi ce document existe

## Contenu Principal
...

## Voir Aussi
- Liens vers docs connexes
```

---

## 📮 Contact et Support

- **Issues GitHub**: Pour bugs et features
- **Discussions**: Pour questions générales
- **Documentation**: Ce dossier !

---

## 🏆 Objectifs du Projet

Exo-OS vise à être un microkernel haute performance avec:

| Objectif | Cible | État |
|----------|-------|------|
| **IPC Latency** | < 500 ns | ⏳ À mesurer |
| **Context Switch** | < 1 µs | ⏳ À mesurer |
| **Syscalls** | > 5M/sec | ⏳ À mesurer |
| **Boot Time** | < 500 ms | ⏳ À mesurer |
| **Threads** | > 1M scalable | ⏳ À implémenter |

**Statut**: Phase 1 - Validation (boot et tests de base)

---

**Dernière mise à jour**: 17 octobre 2025  
**Version du Kernel**: 0.1.0  
**Statut**: En développement actif 🚧
