# Espace de Coordination - workAI

## 🎯 Objectif
Ce dossier sert de point de communication entre les deux IA travaillant sur Exo-OS pour éviter les conflits et les erreurs d'intégration.

## 👥 Répartition des Tâches

### IA #1 (Kernel) - Copilot Principal
**Responsabilité :** Correction du kernel (memory + arch x86_64)
**Zones de travail :**
- `kernel/src/memory/` (tous les fichiers)
- `kernel/src/arch/x86_64/` (tous les fichiers)
- `kernel/src/lib.rs` (exports et configuration kernel)

**Ne PAS toucher :**
- `lib/` (librairie commune)
- `kernel/src/drivers/` (drivers kernel)
- Fichiers dans `workAI/AI2-*`

### IA #2 (Lib + Drivers)
**Responsabilité :** Librairie commune + Drivers kernel
**Zones de travail :**
- `lib/` (toute la librairie commune)
- `kernel/src/drivers/` (tous les drivers)

**Ne PAS toucher :**
- `kernel/src/memory/`
- `kernel/src/arch/`
- Fichiers dans `workAI/AI1-*`

## 📝 Protocole de Communication

### Pour signaler une modification
Créer un fichier : `AI{X}-{date}-{description}.md`

Exemple : `AI1-2025-11-21-nouveau-type-PhysicalAddress.md`

### Format du fichier de signalement
```markdown
# [AI X] Description courte

**Date :** YYYY-MM-DD HH:MM
**Fichiers modifiés :** 
- chemin/vers/fichier1.rs
- chemin/vers/fichier2.rs

## Changements

### Ajout de fonction/type
\`\`\`rust
pub fn nouvelle_fonction() -> Result<(), Error> {
    // ...
}
\`\`\`

### Modification de signature
**Avant :**
\`\`\`rust
pub fn ancienne_signature(param: u32)
\`\`\`

**Après :**
\`\`\`rust
pub fn nouvelle_signature(param: u64) -> Result<(), Error>
\`\`\`

## Impact sur l'autre IA
- [ ] Nécessite mise à jour des imports
- [ ] Nécessite changement d'appels de fonction
- [ ] Pas d'impact

## Notes
Informations supplémentaires...
```

## 🚨 Règles Importantes

1. **Toujours vérifier les fichiers de l'autre IA avant de commencer**
2. **Signaler immédiatement tout changement d'interface publique**
3. **Ne jamais modifier les zones de l'autre IA**
4. **En cas de conflit, créer un fichier `CONFLIT-{description}.md`**

## 📊 État Actuel du Projet

**Dernière compilation :** 267 erreurs
- Modules actifs : `memory`, `arch`
- Modules commentés : `scheduler`, `ipc`, `drivers`, `process`, `syscall`, `boot`

**Objectif immédiat :** Réduire les erreurs dans les modules actifs à 0
