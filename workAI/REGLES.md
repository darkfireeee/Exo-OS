# Règles de Cohabitation - workAI

## 🎯 Principe Fondamental
**Chaque IA a sa zone de responsabilité exclusive. Ne JAMAIS modifier les fichiers de l'autre.**

## 🚨 Zones Interdites par IA

### ❌ IA #1 (Kernel) - NE PAS TOUCHER
- `lib/**` (tout le dossier lib)
- `kernel/src/drivers/**` (tout le dossier drivers)
- `workAI/AI2-*.md` (fichiers de l'autre IA)
- `workAI/TEMPLATE-AI2-*.md` (templates de l'autre IA)

### ❌ IA #2 (Lib + Drivers) - NE PAS TOUCHER  
- `kernel/src/memory/**` (tout le dossier memory)
- `kernel/src/arch/**` (tout le dossier arch)
- `workAI/AI1-*.md` (fichiers de l'autre IA)
- `workAI/TEMPLATE-AI1-*.md` (templates de l'autre IA)

## ✅ Zones Autorisées

### IA #1 (Kernel)
- ✅ `kernel/src/memory/**`
- ✅ `kernel/src/arch/**`
- ✅ `kernel/src/lib.rs` (exports kernel)
- ✅ `workAI/AI1-*.md`
- ✅ `workAI/README.md` (section IA1)

### IA #2 (Lib + Drivers)
- ✅ `lib/**`
- ✅ `kernel/src/drivers/**`
- ✅ `lib/Cargo.toml`
- ✅ `workAI/AI2-*.md`
- ✅ `workAI/README.md` (section IA2)

## 📞 Protocole de Communication

### Quand signaler un changement ?

**IA #2 DOIT signaler si :**
1. Création d'un nouveau type dans `lib/` utilisé par le kernel
2. Modification de signature d'une fonction publique
3. Ajout/suppression d'un export public
4. Changement dans un driver qui expose une nouvelle API
5. Modification de structures de données partagées

**IA #1 DOIT signaler si :**
1. Besoin d'un nouveau type/fonction de `lib/`
2. Besoin d'un nouveau driver ou modification d'API driver
3. Changement d'interface dans `memory/` ou `arch/` qui affecte les drivers
4. Modification des types `PhysicalAddress`, `VirtualAddress`, `PageProtection`

### Comment signaler ?

**Format du nom de fichier :**
```
AI{numéro}-{date}-{description-courte}.md
```

**Exemples :**
- `AI2-2025-11-21-nouveau-driver-nvme.md`
- `AI1-2025-11-21-changement-PhysicalAddress.md`

**Contenu minimal :**
```markdown
# [Titre descriptif]

**Impact :** [Aucun / Mineur / Majeur]
**Fichiers modifiés :** [liste]

## Changement
[Description + code]

## Action requise
[Ce que l'autre IA doit faire, ou "Aucune"]
```

## 🔍 Avant de Commencer une Session

### Checklist IA #1
- [ ] Lire `workAI/AI2-*.md` (nouveaux fichiers depuis dernière session)
- [ ] Vérifier `workAI/CONFLIT-*.md`
- [ ] Mettre à jour `workAI/AI1-STATUS.md`

### Checklist IA #2
- [ ] Lire `workAI/AI1-*.md` (nouveaux fichiers depuis dernière session)
- [ ] Vérifier `workAI/CONFLIT-*.md`
- [ ] Mettre à jour `workAI/AI2-STATUS.md`

## ⚠️ En Cas de Conflit

Si une IA détecte un conflit (ex: erreur de compilation à cause de l'autre) :

1. **NE PAS corriger dans la zone de l'autre IA**
2. Créer un fichier : `CONFLIT-{date}-{description}.md`
3. Y décrire le problème et la solution suggérée
4. Attendre que l'utilisateur arbitre ou que l'autre IA corrige

**Format du fichier conflit :**
```markdown
# 🚨 CONFLIT - [Description]

**Détecté par :** IA #{X}
**Date :** [date]

## Problème
[Description du conflit]

## Cause
[Ce qui a causé le conflit]

## Solution Proposée
[Comment le résoudre]

## Fichiers affectés
- [liste]

---
**Statut :** [ ] Non résolu / [ ] En cours / [ ] Résolu
```

## 📊 Suivi des Modifications

Chaque IA maintient son fichier de statut :
- `AI1-STATUS.md` (IA Kernel)
- `AI2-STATUS.md` (IA Lib+Drivers)

Format du statut :
- Date de dernière modification
- Liste des fichiers modifiés
- Interfaces publiques ajoutées/modifiées
- Problèmes connus
- TODO

## 🎓 Bonnes Pratiques

1. **Toujours vérifier les signaux avant de commencer**
2. **Signaler immédiatement tout changement d'interface**
3. **Être explicite sur les impacts**
4. **En cas de doute, signaler**
5. **Garder les statuts à jour**

## 🔗 Types Partagés Critiques

Ces types sont définis dans `lib/` mais utilisés intensivement par le kernel.
**Toute modification nécessite coordination :**

- `PhysicalAddress`
- `VirtualAddress`
- `PageProtection`
- `PageTableFlags`
- `MemoryError`
- `ArchError`

**Processus pour modifier ces types :**
1. IA #2 crée un signal détaillé
2. IA #1 valide la compatibilité
3. IA #2 implémente
4. IA #1 adapte si nécessaire

---

**Version :** 1.0  
**Dernière mise à jour :** 21 novembre 2025
