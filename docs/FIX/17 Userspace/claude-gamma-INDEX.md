# ExoOS — Audit Userspace/Shell — claude-gamma
## Index des rapports

**Date** : 2026-05-07  
**Scope** : Chemin complet fork → exec → CoW → TTY → shell  
**Contexte** : Suite aux corrections FIX-4/FIX-5 de Codex (GPT-o3) sur les modules `arch`, `memory`, `scheduler`

---

## Fichiers de ce rapport

| Fichier | Contenu |
|---|---|
| `claude-gamma-BUGS-CRITIQUES.md` | Bugs bloquants — aucun shell possible sans corrections |
| `claude-gamma-BUGS-MAJEURS.md` | Bugs importants — shell instable ou partiellement cassé |
| `claude-gamma-PATCHES.md` | Code de correction complet pour chaque bug critique |
| `claude-gamma-EVALUATION-CODEX.md` | Évaluation du chemin pris par Codex (GPT-o3) |

---

## Résumé exécutif

Le terminal/shell ne peut pas démarrer pour **quatre raisons distinctes et indépendantes**, toutes bloquantes :

1. **`KernelFaultAllocator` opère sur le mauvais espace d'adressage** (`KERNEL_AS` au lieu du CR3 du processus courant) → CoW et demand-paging userspace totalement cassés.
2. **Le `VmaTree` n'est pas cloné lors d'un `fork()`** → Tous les processus fils subissent un SEGFAULT immédiat sur leur premier `#PF`.
3. **Les FDs stdin/stdout/stderr ne sont jamais ouverts** → `exosh` appelle `read(0, ...)` / `write(1, ...)` sur des FDs vides.
4. **Aucun pont entre le TTY server (IPC) et les FDs 0/1/2** → Même si les FDs étaient créés, il n'y a rien derrière eux.

Les bugs 1 et 2 sont la cause directe du panic et du blocage après `fork()` que Codex a observé.

---

## Évaluation du chemin Codex

Codex était **sur la bonne piste** pour le bug de cartographie noyau basse (triple fault, CR3 userspace) mais s'est arrêté avant d'atteindre les deux bloqueurs vrais. Les correctifs de mapping kernel et de clone FD (panic `extend_from_slice`) qu'il a appliqués sont valides et nécessaires.
