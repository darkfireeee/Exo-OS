# ✅ CHECKLIST DÉMARRAGE - Exo-OS Session de Travail

**Date:** 4 février 2026  
**Usage:** Vérifier avant chaque session de travail

---

## 🔧 ENVIRONNEMENT

### Vérification Système
```bash
# Aller dans le répertoire projet
cd /workspaces/Exo-OS

# Vérifier git status
git status
# Attendu: Clean working tree ou branches feature clairement identifiées

# Vérifier compilateur Rust
cargo --version
rustc --version
# Attendu: nightly-2024-xx-xx ou plus récent

# Vérifier QEMU
qemu-system-x86_64 --version
# Attendu: QEMU emulator version 6.0+ ou plus
```

### Build Test
```bash
# Clean build
make clean

# Build complet
make build
# Attendu: Compilation sans erreurs

# Vérifier artefacts
ls -lh build/
# Attendu: 
#   - kernel.elf
#   - kernel.bin
#   - exo_os.iso
```

---

## 📚 DOCUMENTATION LIRE

### Avant Jour 1 (exec)
- [ ] REAL_STATE_COMPREHENSIVE_ANALYSIS.md (section Phase 1b)
- [ ] ACTION_PLAN_4_WEEKS.md (Jour 1-2)
- [ ] PREP_JOUR_4-5_EXEC_VFS.md (complet)
- [ ] kernel/src/loader/elf.rs (430 lignes)

### Avant Jour 3 (FD Table)
- [ ] kernel/src/fs/vfs/mod.rs
- [ ] kernel/src/syscall/handlers/io.rs

### Avant Jour 4 (Scheduler)
- [ ] kernel/src/scheduler/mod.rs
- [ ] kernel/src/syscall/handlers/sched.rs

### Avant Jour 5-6 (Signals)
- [ ] kernel/src/syscall/handlers/signals.rs
- [ ] kernel/src/arch/x86_64/ (signal frame ABI)

---

## 🎯 OBJECTIFS SESSION

### Définir Objectif Clair
**Exemple Jour 1:**
```
Objectif: Implémenter load_elf_from_vfs()
Livrable: Fonction qui ouvre fichier via VFS et parse ELF header
Critères: 
  - Compile sans erreurs
  - Open file via VFS réussit
  - Headers parsés correctement
Temps: 4-6h
```

**Template:**
```
Objectif: [Décrire fonction/module]
Livrable: [Artefact concret]
Critères: [Validation objective]
Temps: [Estimation réaliste]
```

---

## 📋 CHECKLIST AVANT CODE

### Compréhension
- [ ] J'ai lu la documentation du module
- [ ] Je comprends l'architecture actuelle
- [ ] J'ai identifié les dépendances
- [ ] Je sais où sont les stubs à remplacer

### Plan
- [ ] J'ai un plan étape par étape
- [ ] Je connais les tests de validation
- [ ] J'ai identifié les bloqueurs potentiels
- [ ] J'ai un plan B si bloqué

### Environnement
- [ ] Code compile actuellement
- [ ] Tests actuels passent
- [ ] Git status clean (ou branche feature)
- [ ] Backup/commit récent existe

---

## 🔨 PENDANT LE CODE

### Règles d'Or
1. **Pas de stub success** - `return 0` INTERDIT sauf si vraiment implémenté
2. **Pas de TODO nouveau** - Implémenter ou ne pas créer
3. **Tests en continu** - Compiler après chaque fonction
4. **Commits atomiques** - Chaque feature = 1 commit

### Checkpoints (Toutes les 30 minutes)
- [ ] Code compile-t-il ?
- [ ] Suis-je toujours sur la bonne piste ?
- [ ] Ai-je besoin d'aide / documentation ?
- [ ] Dois-je faire une pause ?

### Si Bloqué
**Règle 2-4-8:**
- **2h bloqué** → Lire code COMPLET du module
- **4h bloqué** → Rechercher exemples / documentation externe
- **8h bloqué** → Revoir approche / simplifier / demander aide

---

## ✅ CHECKLIST APRÈS CODE

### Validation Code
- [ ] Compile sans erreurs
- [ ] Compile sans warnings (ou warnings documentés)
- [ ] Tests unitaires passent
- [ ] Tests d'intégration passent
- [ ] Performance mesurée (rdtsc si applicable)

### Validation Fonctionnelle
- [ ] Comportement réel vérifié (pas stub)
- [ ] Edge cases testés
- [ ] Pas de régression (anciens tests OK)
- [ ] Documentation à jour

### Git
```bash
# Vérifier changements
git diff

# Vérifier fichiers modifiés
git status

# Commit atomique
git add [fichiers pertinents]
git commit -m "[module]: [description concise]"

# Exemples:
# "exec: Implement VFS loading"
# "io: Connect FD table to VFS"
# "signals: Implement delivery"
```

### Documentation
- [ ] README.md mis à jour si nécessaire
- [ ] Commentaires code ajoutés
- [ ] TODOs supprimés (pas ajoutés!)
- [ ] Métriques mises à jour

---

## 📊 TRACKING PROGRESSION

### Métriques Quotidiennes
```bash
# Compter TODOs restants
grep -r "TODO" kernel/src --include="*.rs" | wc -l

# Compter stubs "return 0"
grep -r "return 0;" kernel/src/syscall/handlers --include="*.rs" -B2 | grep -E "(Stub|TODO)" | wc -l

# Compter tests passants
cargo test 2>&1 | grep "test result"
```

### Tableau de Bord
```
Date: [YYYY-MM-DD]
Jour: [1-28 du plan 4 semaines]
Module: [exec / FD / sched / signals / etc]

TODOs début: [nombre]
TODOs fin: [nombre]
Delta: [+ ou -]

Stubs début: [nombre]
Stubs fin: [nombre]
Delta: [+ ou -]

Tests début: [XX/YY]
Tests fin: [XX/YY]
Delta: [+ ou -]

Temps travail: [Xh]
Temps bloqué: [Yh]
Productivité: [X-Y]h

Commits: [nombre]
LOC ajoutées: [nombre]
LOC supprimées: [nombre]
```

---

## 🎯 VALIDATION FIN DE JOURNÉE

### Questions Clés
1. **Objectif atteint ?**
   - [ ] Oui, livrable complet
   - [ ] Partiellement (spécifier %)
   - [ ] Non (raison?)

2. **Code de qualité ?**
   - [ ] Compile sans erreurs
   - [ ] Pas de stubs ajoutés
   - [ ] Tests passent
   - [ ] Commit fait

3. **Progression mesurable ?**
   - [ ] TODOs réduits
   - [ ] Stubs réduits
   - [ ] Tests augmentés
   - [ ] Documentation à jour

4. **Prêt pour demain ?**
   - [ ] Objectif demain identifié
   - [ ] Documentation lue
   - [ ] Git clean
   - [ ] Mental repos

---

## 🚨 ALERTES

### Red Flags
- ⚠️ **Bloqué >4h sans progrès** → Revoir approche
- ⚠️ **Tests régressent** → Rollback et investiguer
- ⚠️ **Trop de TODOs ajoutés** → Stop, implémenter d'abord
- ⚠️ **Commits trop gros** → Découper en atomiques
- ⚠️ **Stubs augmentent** → Mauvaise direction

### Escalation
Si plusieurs red flags:
1. **STOP coding**
2. **Review plan**
3. **Lire documentation**
4. **Demander aide**
5. **Reprendre avec approche claire**

---

## 📖 RESSOURCES RAPIDES

### Documentation Interne
```
docs/current/
├── REAL_STATE_COMPREHENSIVE_ANALYSIS.md  # État réel détaillé
├── ACTION_PLAN_4_WEEKS.md                # Plan jour par jour
├── EXECUTIVE_SUMMARY.md                  # Synthèse exécutive
└── PREP_JOUR_4-5_EXEC_VFS.md            # Guide exec()

docs/architecture/
├── MEMORY_API_SPEC.md                    # API mémoire
├── VFS_ARCHITECTURE.md                   # Architecture VFS
└── IPC_DESIGN.md                         # Design IPC
```

### Code Référence
```
kernel/src/
├── loader/elf.rs                         # ELF parser
├── fs/vfs/mod.rs                         # VFS API
├── syscall/handlers/                     # Tous les syscalls
├── scheduler/mod.rs                      # Scheduler
└── memory/cow_manager.rs                 # CoW manager
```

### Exemples Externes
- Linux kernel: fs/exec.c (execve)
- Redox OS: kernel/src/syscall/process.rs
- xv6: exec.c (simple reference)

---

## 🎯 TEMPLATE SESSION

### Début Session
```bash
cd /workspaces/Exo-OS
git status
git pull
make clean && make build
cargo test

# Ouvrir:
# - ACTION_PLAN_4_WEEKS.md (jour actuel)
# - Code module à modifier
# - Tests à valider
```

### Pendant Session
```
[Coder selon plan]
[Tester régulièrement]
[Commit atomique]
[Répéter]
```

### Fin Session
```bash
# Validation
cargo test
make build

# Metrics
grep -r "TODO" kernel/src --include="*.rs" | wc -l

# Commit final si nécessaire
git add .
git commit -m "[module]: [résumé]"

# Documentation tracking
echo "Jour X: [module] - TODOs: XX → YY, Stubs: AA → BB" >> docs/current/PROGRESS_LOG.md

# Préparer demain
# - Lire doc jour suivant
# - Identifier objectif
# - Mental repos
```

---

## ✅ PRÊT ?

**Vérifications finales avant démarrer:**

- [ ] Environnement setup (build OK)
- [ ] Documentation lue (jour actuel)
- [ ] Objectif clair défini
- [ ] Plan étape par étape
- [ ] Tests identifiés
- [ ] Git clean
- [ ] Mental focus

**Si tout ✅ → GO CODE! 🚀**

**Si pas tout ✅ → Compléter d'abord, puis GO! 🎯**

---

**Remember:**
> "Code de haute qualité uniquement. Pas de stubs. Pas de TODO. Production-ready code only."

**Let's build! 🔨**
