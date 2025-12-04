# 📚 Exo-OS Documentation

Documentation organisée pour le système d'exploitation Exo-OS v0.5.0 "Linux Crusher"

---

## 📂 Structure

### 📌 `/current/` - Documents actuels et en cours
État actuel du projet, tâches en cours, planification active.

- **PHASE_1_STATUS.md** - ✅ Phase 1 COMPLÈTE: fork/wait cycle fonctionnel
- **PHASE_2_PLAN.md** - 📋 Plan détaillé Phase 2: Fork context copy & POSIX
- **PHASE_2_QUICKSTART.md** - 🚀 Guide rapide démarrage Phase 2
- **ROADMAP.md** - 🗺️ Roadmap v1.0.0 "Linux Crusher"
- **MODULE_STATUS.md** - 📊 État modules kernel
- **TODO.md** - ✅ Liste tâches projet
- **TODO_TECHNIQUE_IMMEDIAT.md** - 🔧 TODOs techniques immédiats

### 🏗️ `/architecture/` - Documentation architecture
Architecture technique, design decisions, analyses système.

- **ARCHITECTURE_v0.5.0.md** - Architecture actuelle v0.5.0
- **ARCHITECTURE_COMPLETE.md** - Architecture complète détaillée
- **IPC_DOCUMENTATION.md** - Inter-Process Communication
- **SCHEDULER_DOCUMENTATION.md** - Scheduler 3-queue EMA
- **POSIX_X_SYSCALL_ANALYSIS.md** - Analyse syscalls POSIX-X

### 📖 `/guides/` - Guides pratiques
Guides de développement, tests, benchmarks.

- **BUILD_AND_TEST_GUIDE.md** - 🔨 Guide compilation et tests
- **AI_INTEGRATION.md** - 🤖 Intégration IA dans Exo-OS
- **exo-os-benchmarks.md** - ⚡ Benchmarks performance

### 🗄️ `/archive/` - Archives historiques
Documentation obsolète conservée pour référence.

#### `/archive/old_versions/` - Anciennes versions
- ARCHITECTURE_v0.4.0.md
- CHANGELOG_v0.4.0.md, v0.5.0.md
- README_old.md, README_v0.4.0.md, README_v0.5.0.md
- roadmap_v0.5.0.md
- v0.5.0_RELEASE_NOTES.md
- exo-os avancé.txt

#### `/archive/completed_tasks/` - Tâches terminées
- COMPILATION_SUCCESS.md
- HEAP_ALLOCATOR_FIX.md
- LINKAGE_SUCCESS_REPORT.md
- SESSION_SUMMARY.md
- TRAVAIL_TERMINE.md

### 📁 Dossiers modules spécifiques
Documentation détaillée par composant système.

- `/ipc/` - IPC module documentation
- `/loader/` - ELF loader documentation
- `/memory/` - Memory management
- `/scheduler/` - Scheduler internals
- `/vfs/` - Virtual File System
- `/x86_64/` - Architecture x86_64
- `/scripts/` - Scripts documentation

---

## 🚀 Quick Start

### Pour commencer le développement:
1. **[BUILD_AND_TEST_GUIDE.md](guides/BUILD_AND_TEST_GUIDE.md)** - Compiler et tester
2. **[PHASE_2_QUICKSTART.md](current/PHASE_2_QUICKSTART.md)** - Démarrer Phase 2

### Pour comprendre l'architecture:
1. **[ARCHITECTURE_v0.5.0.md](architecture/ARCHITECTURE_v0.5.0.md)** - Vue d'ensemble
2. **[SCHEDULER_DOCUMENTATION.md](architecture/SCHEDULER_DOCUMENTATION.md)** - Scheduler
3. **[IPC_DOCUMENTATION.md](architecture/IPC_DOCUMENTATION.md)** - IPC

### Pour voir l'état actuel:
1. **[PHASE_1_STATUS.md](current/PHASE_1_STATUS.md)** - Phase 1 complète ✅
2. **[MODULE_STATUS.md](current/MODULE_STATUS.md)** - État modules
3. **[ROADMAP.md](current/ROADMAP.md)** - Plan v1.0.0

---

## 📊 État Projet

**Version actuelle**: v0.5.0 "Linux Crusher"  
**Phase**: Phase 1 ✅ COMPLÈTE | Phase 2 🚧 EN COURS

### Phase 1 - fork/exec/wait (✅ COMPLETE)
- ✅ fork() crée processus enfants
- ✅ exit() zombie state
- ✅ wait() collecte et reap zombies
- ✅ Tests passent: getpid, fork, fork_wait_cycle

### Phase 2 - Context Copy & POSIX (🚧 EN COURS)
- [ ] Fork retourne 0 dans enfant
- [ ] Thread context copy complet
- [ ] Test exec() avec ELF réel
- [ ] Fork+exec+wait intégration

---

## 🔗 Liens Rapides

- **Main README**: [../README.md](../README.md)
- **Index complet**: [INDEX.md](INDEX.md)
- **Benchmarks**: [guides/exo-os-benchmarks.md](guides/exo-os-benchmarks.md)

---

*Documentation mise à jour: 2024-12-04*  
*Exo-OS - "We're not just compatible with Linux, we're crushing it"*
