# 🎯 PRÉPARATION JOUR 4-5 : exec() VFS Integration

**Date Préparation**: 2026-01-03  
**Exécution Prévue**: Jour 4-5 (2 jours)  
**Objectif**: Charger binaires ELF depuis VFS au lieu de stubs

---

## 📋 ANALYSE PRÉLIMINAIRE

### État Actuel

**Problème Identifié** (REAL_STATE_ANALYSIS.md):

```rust
// kernel/src/syscall/handlers/process.rs
// Note: Currently using a stub - real impl needs VFS file reading

// kernel/src/loader/elf.rs
// TODO: Utiliser des mappings temporaires
.map_err(|_| ElfError::InvalidProgramHeader)?; // TODO: Better error
```

### Fichiers Clés à Analyser

1. **kernel/src/loader/elf.rs**
   - Parser ELF existant
   - Structures ELF headers
   - load_elf() actuel

2. **kernel/src/syscall/handlers/process.rs**
   - sys_execve() actuel
   - Comment stub charge binaire

3. **kernel/src/fs/** (VFS layer)
   - Comment ouvrir fichier
   - Comment lire contenu
   - API disponibles

4. **kernel/src/loader/spawn.rs**
   - spawn_process() actuel
   - Address space setup

---

## 🎯 OBJECTIFS JOUR 4

### Objectif Principal
**Remplacer stub exec() par VFS loading réel**

### Critères de Succès

| Critère | Validation |
|---------|-----------|
| 1. exec() ouvre fichier via VFS | `vfs::open(path)` appelé |
| 2. ELF headers lus depuis fichier | Headers parsés correctement |
| 3. Segments PT_LOAD mappés | Mémoire allouée + mappée |
| 4. Tests loading basique | 2/4 tests passent |

### Tâches Planifiées

**Phase 1: Analyse** (2h)
- [ ] Lire kernel/src/loader/elf.rs complet
- [ ] Lire kernel/src/syscall/handlers/process.rs
- [ ] Identifier API VFS pour open/read
- [ ] Comprendre stub actuel

**Phase 2: Implémentation** (4h)
- [ ] Créer load_elf_from_vfs(path: &str)
- [ ] Ouvrir fichier via VFS
- [ ] Lire ELF header (52 bytes)
- [ ] Valider magic ELF (0x7f 'E' 'L' 'F')
- [ ] Lire program headers
- [ ] Parser PT_LOAD segments

**Phase 3: Mapping** (2h)
- [ ] Allouer pages pour chaque PT_LOAD
- [ ] Mapper virt → phys addresses
- [ ] Copier données depuis fichier
- [ ] Set permissions (RWX)

**Phase 4: Tests** (2h)
- [ ] Créer test binaire simple (test_hello)
- [ ] Test 1: exec() ouvre fichier ✅
- [ ] Test 2: ELF headers parsés ✅
- [ ] Documentation Jour 4

---

## 🎯 OBJECTIFS JOUR 5

### Objectif Principal
**Compléter exec() avec argv/envp + tests avancés**

### Tâches Planifiées

**Phase 1: Stack Setup** (3h)
- [ ] Setup stack utilisateur
- [ ] Push argv strings
- [ ] Push envp strings
- [ ] Setup argc/argv/envp pointers
- [ ] Align stack (16 bytes)

**Phase 2: PT_INTERP Support** (2h)
- [ ] Détecter PT_INTERP segment
- [ ] Charger dynamic linker si présent
- [ ] Setup auxiliary vectors

**Phase 3: Tests Complets** (3h)
- [ ] Test 3: argv/envp correctement passés ✅
- [ ] Test 4: exec() + fork() combiné ✅
- [ ] Test edge cases (fichier invalide, etc.)
- [ ] Benchmark latency exec()

**Phase 4: Documentation** (1h)
- [ ] Documentation Jour 5
- [ ] Commit final

---

## 📝 QUESTIONS À RÉSOUDRE

### Questions Techniques

1. **VFS API**:
   - Quelle fonction pour ouvrir fichier?
   - Comment lire N bytes depuis FD?
   - Gestion erreurs VFS?

2. **Memory Mapping**:
   - Utiliser cow_manager pour mapping?
   - Comment allouer pages anonymes vs file-backed?
   - TLB flush nécessaire?

3. **Address Space**:
   - Réutiliser address_space actuel ou nouveau?
   - Cleanup ancien address space du process?
   - Stack position (0x7fff...)?

4. **Dynamic Linker**:
   - PT_INTERP obligatoire ou optionnel?
   - Loader où (/lib/ld-linux.so.2)?
   - Comment passer contrôle au linker?

### Dépendances

**Requis**:
- ✅ CoW Manager (Jour 2)
- ✅ Page fault handler (Jour 3)
- ❓ VFS file reading (à vérifier)
- ❓ Address space management (à vérifier)

**Bloqueurs Potentiels**:
- VFS read() non fonctionnel
- Page allocator issues
- TLB invalidation manquante

---

## 🔍 ANALYSE DE CODE À FAIRE

### Fichiers à Lire en Détail

```bash
# Loader ELF
kernel/src/loader/elf.rs         # Parser ELF existant
kernel/src/loader/spawn.rs       # Spawn process
kernel/src/loader/mod.rs         # Module loader

# Syscalls
kernel/src/syscall/handlers/process.rs   # sys_execve()

# VFS
kernel/src/fs/mod.rs             # VFS root
kernel/src/fs/vfs.rs             # VFS operations
libs/fs/                         # FS libs

# Memory
kernel/src/memory/virtual_mem/mod.rs     # Address space
kernel/src/memory/physical_mem.rs        # Frame allocator
```

### Grep Patterns Utiles

```bash
# Trouver exec() actuel
grep -r "execve" kernel/src/syscall/

# Trouver VFS open
grep -r "fn open" kernel/src/fs/

# Trouver ELF loading
grep -r "load_elf" kernel/src/loader/

# Trouver TODOs exec
grep -r "TODO.*exec" kernel/
```

---

## 📊 MÉTRIQUES OBJECTIFS

### Jour 4

| Métrique | Objectif |
|----------|----------|
| LOC ajoutées | ~150-200 |
| Tests passés | 2/4 (50%) |
| TODOs éliminés | 2-3 |
| Fichiers modifiés | 3-4 |

### Jour 5

| Métrique | Objectif |
|----------|----------|
| LOC ajoutées | ~100-150 |
| Tests passés | 4/4 (100%) |
| TODOs éliminés | 2-3 |
| Latency exec() | <5ms |

### Total Jours 4-5

- **LOC**: +250-350
- **Tests**: 4/4 (100%)
- **TODOs**: -5
- **Commits**: 2 (Jour 4 + Jour 5)

---

## ✅ CHECKLIST AVANT DE COMMENCER

### Préparation Environnement

- [ ] Git status clean (commit Jour 3 fait)
- [ ] Tests Jour 3 passent (cargo test)
- [ ] Documentation à jour
- [ ] QEMU fonctionnel pour tests

### Préparation Mentale

- [ ] Objectif clair: VFS loading ELF
- [ ] Plan détaillé lu
- [ ] Questions identifiées
- [ ] Backup plan si bloqué

### Outils Nécessaires

- [ ] Éditeur configuré
- [ ] Debugger prêt
- [ ] hexdump pour vérifier ELF
- [ ] readelf pour tests

---

## 🚧 PLAN B (SI BLOQUÉ)

### Si VFS read() non fonctionnel

**Option 1**: Implémenter VFS read() minimal
- Juste pour files FAT32
- Read N bytes depuis offset
- Retour buffer

**Option 2**: Stub amélioré
- Lire depuis initramfs
- Hardcoded test binaries
- Juste pour tests

**Option 3**: Report Jour 6
- Focaliser sur VFS d'abord
- exec() après VFS stable

### Si Memory mapping problématique

**Option 1**: Utiliser mappings existants
- Réutiliser code actuel
- Juste adapter pour ELF

**Option 2**: Mapping simplifié
- Pas de CoW pour segments PT_LOAD
- Direct phys allocation

**Option 3**: Ask for help
- Analyser avec subagent
- Recherche exemples Linux

---

## 📚 RESSOURCES

### Documentation

- ELF Specification: https://refspecs.linuxfoundation.org/elf/elf.pdf
- Linux execve man page
- Docs internes: docs/loader/, docs/memory/

### Code Référence

- Linux kernel: fs/exec.c
- Redox OS: kernel/src/syscall/process.rs
- seL4: libsel4/arch_include/x86/

### Tests

- Binaires test existants: userland/test_*.c
- Scripts build: scripts/compile_userland.sh

---

## 🎯 DÉFINITION OF DONE

### Jour 4 Terminé Quand:

- [x] exec() peut charger test_hello depuis VFS
- [x] ELF headers correctement parsés
- [x] Segments PT_LOAD mappés en mémoire
- [x] 2 tests passent
- [x] Code compile sans warnings
- [x] Documentation Jour 4 créée
- [x] Commit atomique fait

### Jour 5 Terminé Quand:

- [x] argv/envp passés correctement
- [x] fork() + exec() fonctionne
- [x] 4/4 tests passent
- [x] Latency <5ms
- [x] Code sans TODOs critiques
- [x] Documentation Jour 5 créée
- [x] Commit atomique fait

---

## 📞 POINTS DE SYNCHRONISATION

### Checkpoints Jour 4

| Temps | Checkpoint | Action si retard |
|-------|-----------|------------------|
| +2h | Analyse terminée | Skip détails, focus impl |
| +6h | VFS loading OK | Continuer mapping |
| +8h | Mapping basique | Tests demain |
| +10h | Tests 2/4 | Documentation + commit |

### Checkpoints Jour 5

| Temps | Checkpoint | Action si retard |
|-------|-----------|------------------|
| +3h | Stack setup OK | Tests simples d'abord |
| +5h | PT_INTERP support | Optionnel, skip si retard |
| +8h | Tests 4/4 | Debugging si fails |
| +10h | Docs + commit | Priorité absolue |

---

## 🔄 LIEN AVEC PLAN GLOBAL

### Semaine 1 Status

```
✅ Jour 1: Analyse (2026-01-02)
✅ Jour 2: CoW Manager (2026-01-02)
✅ Jour 3: Page Fault Integration (2026-01-03)
🔄 Jour 4-5: exec() VFS Integration (SUIVANT)
⏳ Jour 6-7: Process Cleanup
⏳ Jour 8: Signal Delivery
```

**Progression Semaine 1**: 3/8 jours (37.5%)

### Impact sur Objectif Semaine 1

**Objectif**: fork/exec/wait fonctionnels

Après Jour 5:
- fork() ✅ (avec CoW complet)
- exec() ✅ (avec VFS loading)
- wait() ⏳ (Jour 6-7)
- signals ⏳ (Jour 8)

**Progression Attendue**: 60% semaine 1

---

**Préparé le**: 2026-01-03  
**Pour exécution**: Jour 4 (prochain)  
**Auteur**: GitHub Copilot  
**Status**: ✅ Prêt à démarrer
