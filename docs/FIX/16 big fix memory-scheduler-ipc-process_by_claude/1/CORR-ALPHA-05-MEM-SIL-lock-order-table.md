# CORR-ALPHA-05 — Memory : Table lock order inversée vs Architecture v7

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Classe :** 🟠 SIL — Incohérence critique entre spécifications  
> **Fichier :** `docs/kernel/memory/MEMORY_COMPLETE.md §2`  
> **Règle :** `LOCK ORDER`  
> **Sévérité :** Majeure — un développeur lisant uniquement la doc memory pourrait acquérir les locks dans le mauvais ordre → deadlock SMP

---

## 1. Description du bug

### 1.1 Règle dans MEMORY_COMPLETE.md §2

```markdown
| **LOCK ORDER** | IPC < Scheduler < **Memory** < FS (jamais lock N si on tient N+1). |
```

Cette notation avec `<` placé entre les modules indique un ordre d'acquisition : IPC en premier, puis Scheduler, puis Memory, puis FS.

### 1.2 Ordre canonique dans Architecture v7 §2.2

```markdown
| Niveau | Module | Règle |
|--------|--------|-------|
| 1 (acquérir en premier) | Memory | Jamais tenus lors d'appels aux couches supérieures |
| 2 | Scheduler | RunQueue locks (ordre cpu_id croissant) |
| 3 | Security | CapTable locks |
| 4 | IPC | Channel/endpoint/SHM |
| 5 (acquérir en dernier) | FS | DOIT relâcher AVANT tout appel IPC |
```

L'ordre canonique est : **Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5)**.

### 1.3 Analyse de la contradiction

| Module | MEMORY_COMPLETE | Architecture v7 |
|--------|----------------|----------------|
| IPC | Niveau 1 (premier) | Niveau 4 (avant-dernier) |
| Scheduler | Niveau 2 | Niveau 2 ✅ |
| Memory | Niveau 3 | Niveau 1 ✅ inversé |
| FS | Niveau 4 (dernier) | Niveau 5 (dernier) ✅ |

**Memory est au niveau 3 dans MEMORY_COMPLETE** mais devrait être au niveau 1 (acquis en premier).  
**IPC est au niveau 1 dans MEMORY_COMPLETE** mais devrait être au niveau 4 (acquis après Memory, Scheduler, Security).

### 1.4 Interprétation du problème

La confusion vient probablement de deux conventions différentes :
- La doc memory utilise `<` avec la signification "acquiert avant" (IPC < Scheduler → IPC acquis avant Scheduler)
- Mais du point de vue de memory/, la règle réelle est "je suis acquis en premier" (Memory est la fondation)

La mémoire n'appelle jamais le scheduler ni l'IPC. Si un thread tient un lock Memory et essaie d'appeler IPC, c'est une violation. Mais selon l'ordre canonique, c'est Memory qui doit être acquis EN PREMIER — pas IPC.

La doc memory semble décrire l'ordre vu "de haut en bas" (IPC au sommet → FS à la base), alors que l'ordre canonique est "de bas en haut" (Memory à la base → FS au sommet de la pile de dépendances).

---

## 2. Correctif

### Fichier : `docs/kernel/memory/MEMORY_COMPLETE.md §2`

**Avant :**
```markdown
| **LOCK ORDER** | IPC < Scheduler < **Memory** < FS (jamais lock N si on tient N+1). |
```

**Après :**
```markdown
| **LOCK ORDER** | Ordre d'acquisition : **Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5)**. Memory est acquis EN PREMIER. Jamais acquérir un lock de niveau N si on tient déjà un lock de niveau N+1 ou supérieur. Règle mémoire : `memory/` ne tient jamais un lock lors d'un appel à `scheduler/`, `ipc/`, `fs/`. |
```

### Reformulation complète du tableau des règles §2

Remplacer la ligne LOCK ORDER par :

```markdown
| **LOCK ORDER** | Hiérarchie canonique (Architecture v7 §2.2) : Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5). Acquérir dans l'ordre croissant. Ne jamais acquérir un lock de niveau inférieur si on tient un lock de niveau supérieur (ex. : ne pas acquérir un spinlock memory/ si on tient un lock ipc/). FS DOIT relâcher ses locks AVANT tout appel IPC (FS bloquant). |
```

---

## 3. Vérification des autres occurrences

### Dans `kernel/src/memory/mod.rs` (commentaire)

```rust
// Ordonnancement des locks : Memory → Scheduler → Security → IPC → FS.
```
✅ **Correct** — le code source a le bon ordre.

### Dans `kernel/src/ipc/mod.rs` (commentaire)

```rust
// Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5)
// NE JAMAIS acquérir un lock Memory/Scheduler dans un contexte IPC verrouillé.
```
✅ **Correct** — le code source a le bon ordre.

### Conclusion

Le code source est correct. Seule `docs/kernel/memory/MEMORY_COMPLETE.md §2` a l'ordre inversé dans sa table de règles.

---

## 4. Impact scope

- **Fichier modifié :** `docs/kernel/memory/MEMORY_COMPLETE.md §2` uniquement
- **Nature :** correction de documentation
- **Aucun changement de comportement runtime**
- **Risque si non corrigé :** un développeur junior lisant uniquement cette doc et non Architecture v7 pourrait tenter d'acquérir un lock IPC avant un lock Memory → deadlock SMP garanti en scénario de pression mémoire + communication IPC simultanées

---

*— claude-alpha*
