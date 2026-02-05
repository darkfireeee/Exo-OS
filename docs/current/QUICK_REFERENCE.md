# 🎯 QUICK REFERENCE - Exo-OS Real Implementation

**Usage:** Référence rapide pendant le code  
**Date:** 2026-02-04

---

## 📊 ÉTAT ACTUEL (BASELINE)

```
Global:     35-40% fonctionnel réel (pas 58% annoncé)
Phase 1:    45% (exec stub, FD déconnecté)
Phase 2:    22% (network 90% stub)
Phase 3:    5% (drivers absents)

TODOs:      200-250
Stubs:      97 critiques
Tests:      50/60 (acceptent stubs)
```

---

## 🎯 OBJECTIFS 4 SEMAINES

```
Global:     35% → 80%
Phase 1:    45% → 95%
Phase 2:    22% → 70%
Phase 3:    5% → 55%

TODOs:      200 → <30
Stubs:      97 → <10
Tests:      50 → 80 (tests réels)
```

---

## 📅 PLAN SEMAINE 1 (Jours 1-7)

### Priorités
```
P0 - CRITIQUE:
  ✓ Jour 1-2: exec() VFS Loading
  ✓ Jour 3:   FD Table → VFS

P1 - IMPORTANT:
  ✓ Jour 4:   Scheduler Syscalls
  ✓ Jour 5-6: Signal Delivery

P2 - SOUHAITABLE:
  ✓ Jour 7:   Process Limits
```

### Livrables
```
- [ ] exec() charge depuis VFS (pas stub)
- [ ] open/read/write connectés VFS
- [ ] sched_yield() appelle scheduler
- [ ] kill() délivre signal réel
- [ ] Resource limits trackés
```

---

## 🔴 STUBS CRITIQUES À ÉLIMINER

### Semaine 1
```
❌ sys_execve() → load_elf_from_stub()
❌ sys_open() → return -1
❌ sys_read() → return 0 (stub)
❌ sys_sched_yield() → return 0 (stub)
❌ sys_kill() → return 0 (stub)
❌ sys_getrlimit() → hardcoded values
```
### Semaine 2 (IPC)
```
❌ fusion_rings_create() → handles fake
❌ sys_send() → Ok(len) (stub)
❌ sys_shmget() → fake ID
```

### Semaine 3 (Storage)
```
❌ Drivers block absents
❌ FAT32 parser non utilisé
❌ ext4 absent
```
### Semaine 4 (Network)
```
❌ send_segment() → Ok(()) (stub)
❌ send_to() → Ok(len) (données perdues)
❌ arp_resolve() → Err(Timeout)
```

---

## ✅ RÈGLES D'OR

### Code
1. **ZÉRO stub success** - Pas de `return 0` fake
2. **ZÉRO TODO nouveau** - Implémenter ou ne pas créer
3. **Tests réels** - Vérifier comportement, pas retour
4. **Commits atomiques** - 1 feature = 1 commit

### Workflow
```
1. Lire doc module (30min minimum)
2. Comprendre architecture actuelle
3. Plan étape par étape
4. Coder + tester continuellement
5. Commit atomique
6. Documentation à jour
```

### Si Bloqué
```
2h  → Lire code COMPLET module
4h  → Recherche exemples externes
8h  → Revoir approche / demander aide
```

---

## 📂 FICHIERS CLÉS

### Documentation
```
docs/current/
├── REAL_STATE_COMPREHENSIVE_ANALYSIS.md   # État réel détaillé
├── ACTION_PLAN_4_WEEKS.md                 # Plan jour par jour
├── EXECUTIVE_SUMMARY.md                   # Synthèse
├── STARTUP_CHECKLIST.md                   # Checklist session
├── PROGRESS_LOG.md                        # Tracking
└── QUICK_REFERENCE.md                     # Ce fichier
```

### Code Modules
```
kernel/src/
├── loader/elf.rs                    # ELF parser (430 lignes)
├── fs/vfs/mod.rs                    # VFS API
├── syscall/handlers/
│   ├── process.rs                   # fork/exec/wait (1080 lignes)
│   ├── io.rs                        # open/read/write
│   ├── sched.rs                     # scheduler syscalls
│   └── signals.rs                   # signal handling
├── scheduler/mod.rs                 # Scheduler core
└── memory/cow_manager.rs            # CoW (393 lignes)
```

---

## 🔧 COMMANDES UTILES

### Build
```bash
cd /workspaces/Exo-OS
make clean
make build
cargo test
```

### Métriques
```bash
# TODOs
grep -r "TODO" kernel/src --include="*.rs" | wc -l

# Stubs
grep -r "return 0;" kernel/src/syscall/handlers --include="*.rs" -B2 | grep -E "(Stub|TODO)" | wc -l

# LOC
find kernel/src -name "*.rs" -exec wc -l {} + | tail -1
```

### Git
```bash
git status
git diff
git add [files]
git commit -m "[module]: [description]"
```

### QEMU
```bash
# Test rapide
./scripts/test_qemu.sh

# Debug
qemu-system-x86_64 -cdrom build/exo_os.iso -m 128M -nographic -serial mon:stdio
```

---

## 📋 TEMPLATE JOUR

### Début
```bash
cd /workspaces/Exo-OS
git status
make clean && make build
cargo test

# Ouvrir:
# - ACTION_PLAN_4_WEEKS.md (jour X)
# - Module à coder
# - Tests à valider
```

### Pendant
```
[Objectif du jour défini]
[Plan étape par étape]
[Coder + tester]
[Commit atomique]
```

### Fin
```bash
cargo test
make build

# Métriques
grep -r "TODO" kernel/src --include="*.rs" | wc -l

# Commit
git commit -m "[résumé]"

# Log
echo "Jour X: [module] - TODOs: XX→YY" >> docs/current/PROGRESS_LOG.md
```

---

## 🎯 VALIDATION CRITIQUE

### exec() VFS Loading (Jour 1-2)
```
✓ Ouvre fichier via VFS (pas hardcodé)
✓ Parse ELF header
✓ Map segments PT_LOAD
✓ Setup stack argv/envp
✓ Programme s'exécute

Test:
  execve("/bin/test", ...) → affiche "Hello VFS!"
```

### FD Table VFS (Jour 3)
```
✓ open() → VFS::open()
✓ read() → VFS::read()
✓ write() → VFS::write()

Test:
  fd = open("/dev/zero", O_RDONLY)
  read(fd, buf, 16) → buf = [0,0,0,...]
```

### Scheduler Syscalls (Jour 4)
```
✓ sched_yield() → SCHEDULER.yield_cpu()
✓ nice() → adjust_priority()

Test:
  tid_before = gettid()
  sched_yield()
  tid_after = gettid()
  assert(tid_before != tid_after)  // Context switch réel
```

### Signal Delivery (Jour 5-6)
```
✓ kill() → enqueue signal
✓ deliver_pending_signals()
✓ setup_signal_frame()
✓ sys_sigreturn()

Test:
  kill(pid, SIGINT) → handler appelé avec sig=2
  sigreturn() → context restauré
```

---

## 🚨 RED FLAGS

### STOP si:
- ⚠️ Bloqué >4h sans progrès
- ⚠️ Tests régressent
- ⚠️ TODOs augmentent (au lieu de baisser)
- ⚠️ Stubs ajoutés
- ⚠️ Commits trop gros (>500 LOC)

### Action:
1. STOP coding
2. Review plan
3. Lire documentation
4. Revoir approche
5. Reprendre avec clarté

---

## 📊 MÉTRIQUES CIBLES

### Fin Semaine 1
```
Phase 1:     80% (objectif)
TODOs:       <150 (objectif)
Stubs:       <60 (objectif)
Tests:       65/60 (objectif)
Commits:     ~7
LOC:         +2000-3000
```

### Fin 4 Semaines
```
Global:      80%
Phase 1:     95%
Phase 2:     70%
Phase 3:     55%
TODOs:       <30
Stubs:       <10
Tests:       80/60
```

---

## 💡 RAPPELS IMPORTANTS

### Philosophie
> "Code production uniquement. Zéro stub. Zéro fake success."

### Focus
```
Semaine 1: Phase 1 complet (exec, FD, signals)
Semaine 2: Network fonctionnel
Semaine 3: Storage fonctionnel
Semaine 4: IPC + finition
```

### Qualité > Quantité
```
Mieux vaut:
  - 1 feature 100% fonctionnelle
  - Que 10 features 50% stub
```

---

## 🔗 LIENS RAPIDES

### Ressources
- [Linux exec.c](https://github.com/torvalds/linux/blob/master/fs/exec.c)
- [Redox syscalls](https://github.com/redox-os/kernel/tree/master/src/syscall)
- [xv6 exec](https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/exec.c)

### Documentation
- ELF Spec: https://refspecs.linuxfoundation.org/elf/elf.pdf
- System V ABI: https://wiki.osdev.org/System_V_ABI
- Signal Frame: https://en.wikipedia.org/wiki/Sigcontext

---

## ✅ CHECKPOINT RAPIDE

**Avant de coder:**
- [ ] Documentation lue ?
- [ ] Objectif clair ?
- [ ] Plan étape par étape ?
- [ ] Tests identifiés ?

**Pendant le code:**
- [ ] Compile ?
- [ ] Pas de stub ajouté ?
- [ ] Tests passent ?

**Après le code:**
- [ ] Validation complète ?
- [ ] Commit atomique ?
- [ ] Documentation à jour ?
- [ ] Métriques trackées ?

---

**Ready? GO! 🚀**

**Remember:** Haute qualité, zéro compromis, code production uniquement.
