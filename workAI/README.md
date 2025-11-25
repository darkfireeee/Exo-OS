# 🤖 WorkAI - Espace de Collaboration IA

## 📋 Objectif
Reconstruction complète du kernel Exo-OS avec code optimisé et architecture moderne.

## 👥 Équipe IA
- **Copilot** (GitHub/Claude) : Zones critiques + coordination
- **Gemini** (Google/Antigravity) : Zones support + implémentation

## 📊 État Global

### Statistiques
- **Zones totales** : 12
- **Zones critiques** : 6 (Copilot)
- **Zones support** : 6 (Gemini)
- **Progression** : 0% (Démarrage)

### Zones Critiques (Copilot)
1. ✅ Boot & Architecture (arch/x86_64/boot/)
2. ⏳ Memory Management (memory/)
3. ⏳ IPC Fusion Rings (ipc/)
4. ⏳ Scheduler (scheduler/)
5. ⏳ Syscalls (syscall/)
6. ⏳ Security Core (security/)

### Zones Support (Gemini)
1. ⏳ Drivers Base (drivers/)
2. ⏳ Filesystem (fs/)
3. ⏳ Network Stack (net/)
4. ⏳ POSIX-X Layer (posix_x/)
5. ⏳ AI Agents (ai/)
6. ⏳ Utils & Tests (utils/, tests/)

## 🔄 Workflow

### Copilot (Coordinateur)
1. Créer la structure de base du kernel
2. Implémenter zones critiques
3. Définir les interfaces pour Gemini
4. Valider le code de Gemini
5. Intégration finale

### Gemini (Implémenteur)
1. Lire les interfaces définies par Copilot
2. Implémenter zones support selon specs
3. Tester individuellement
4. Signaler problèmes dans STATUS
5. Soumettre pour review

## 📂 Structure des Fichiers

```
workAI/
├── README.md                    # Ce fichier
├── STATUS_COPILOT.md            # État Copilot (mis à jour par Copilot)
├── STATUS_GEMINI.md             # État Gemini (mis à jour par Gemini)
├── INTERFACES.md                # Interfaces définies par Copilot
├── DIRECTIVES.md                # Directives techniques partagées
├── PROBLEMS.md                  # Problèmes rencontrés
└── PROGRESS.md                  # Progression globale
```

## 🎯 Règles de Collaboration

### Communication
- **Chaque IA met à jour son STATUS toutes les 30min**
- **Signaler IMMÉDIATEMENT les blocages dans PROBLEMS.md**
- **Ne jamais modifier le code de l'autre sans coordination**
- **Respecter les interfaces définies dans INTERFACES.md**

### Qualité Code
- **Rust** : rustfmt + clippy level=pedantic
- **C** : clang-format style=kernel
- **ASM** : NASM syntax, commentaires obligatoires
- **Tests** : Minimum 80% coverage par zone

### Performance
- **Zero-copy partout où possible**
- **Pas d'allocations dans fast path**
- **Mesurer avec rdtsc pour optimisations**
- **Benchmarks vs objectifs (voir exo-os-benchmarks.md)**

## 🚀 Démarrage

### Phase 1 : Structure (Jour 1)
- [ ] Copilot : Créer arborescence kernel/
- [ ] Copilot : boot.asm + boot.c fonctionnels
- [ ] Copilot : Définir interfaces principales
- [ ] Gemini : Lire INTERFACES.md
- [ ] Gemini : Préparer structure drivers/

### Phase 2 : Implémentation (Jours 2-7)
- [ ] Zones critiques par Copilot (parallèle)
- [ ] Zones support par Gemini (parallèle)
- [ ] Reviews croisées quotidiennes

### Phase 3 : Intégration (Jours 8-10)
- [ ] Intégration progressive
- [ ] Tests end-to-end
- [ ] Benchmarks validation
- [ ] Documentation

## 📞 Contact d'Urgence
Si problème bloquant : Signaler dans PROBLEMS.md avec tag [URGENT]
