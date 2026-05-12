# claude-iota — Rapport d'analyse Exo-OS (Shell/Userspace)

**Date** : 2026-05-07  
**Analyste** : Claude (Sonnet 4.6)  
**Périmètre** : Blocage démarrage terminal/shell — fork, CoW, exec, init_server

---

## Résumé exécutif

Le shell ne démarre pas car **deux bugs structurels bloquants** empêchent tout processus forké de survivre à sa première page fault.  
Les corrections de Codex (gpt5.5) ont résolu des problèmes réels (triple fault KPTI, panic FD-clone, TLB flush) mais ont contourné sans résoudre le nœud central : **l'arbre VMA n'est jamais cloné au fork, et `mark_vma_cow()` n'est jamais appelé**.

---

## Fichiers de rapport

| Fichier | Contenu |
|---------|---------|
| `claude-iota-bug-P0-vma-clone.md` | **[BLOQUANT]** VMA tree non cloné au fork |
| `claude-iota-bug-P0-mark-vma-cow.md` | **[BLOQUANT]** VmaFlags::COW jamais positionné |
| `claude-iota-bug-P1-execve-postdispatch.md` | [HAUT] post_dispatch absent après execve raté |
| `claude-iota-bug-P1-rflags-mask.md` | [HAUT] RFLAGS_FORCE_CLR masque faux |
| `claude-iota-bug-P2-ipc-ready.md` | [MOYEN] Fausse détection IPC readiness |
| `claude-iota-fix-plan.md` | Plan de correction ordonné |

---

## Séquence de crash actuelle

```
PID1 (init_server)
  └─ fork() ──────────────────────────────────────► [kernel do_fork]
        │  clone PTE en CoW (FLAG_COW + read-only)   │
        │  VMA tree du fils = VIDE                   │
        │  mark_vma_cow() jamais appelé              │
        ▼                                            ▼
  parent SYSRETQ ◄─ retour rax=child_pid       fils schedulé
        │                                            │
        │ 1ère écriture sur pile                    │ 1ère écriture sur pile
        │ #PF Write, VMA trouvée (WRITE flag)        │ #PF Write, VMA = None
        │ VmaFlags::COW absent                       │ SIGSEGV
        │ → demand_paging (WRONG PATH)               │ → child killed
        │ → alloue new frame sur page présente       │
        │ → soit OOM, soit data corruption           │
        ▼                                            
  parent survit aléatoirement                   
  wait4(child) → child mort → ECHILD            
  retry fork → boucle infinie                   
```

---

## Verdict sur les corrections Codex

| Correction Codex | Statut | Note |
|-----------------|--------|------|
| Cartographie noyau low (triple fault) | ✅ Correcte | Résout le #GP/triple fault au boot |
| Clone de FD table (panic slice) | ✅ Correcte | Remplace extend_from_slice par boucle fallible |
| fs_bridge NotReady → EAGAIN | ✅ Correcte | Sémantique correcte |
| Doctest kernel désactivés | ✅ Correcte | Fix harness host/bare-metal |
| Instrumentation do_fork (fork_trace) | ✅ Utile | Debug restant mais non nocif |
| Orientation vers CoW stack parent | ⚠️ Partielle | Identifie le symptôme, pas la cause racine |
| Aucune correction VMA tree | ❌ Manquante | **Bug bloquant non traité** |
| Aucun appel mark_vma_cow | ❌ Manquante | **Bug bloquant non traité** |
