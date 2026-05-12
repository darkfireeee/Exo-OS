# ExoOS — Évaluation du chemin Codex (GPT-o3) — claude-gamma
## Fichier : claude-gamma-EVALUATION-CODEX.md

---

## Résumé de l'évaluation

**Verdict : Codex était sur la bonne piste pour les symptômes observés, mais s'est arrêté avant les deux causes racines.**

---

## Ce que Codex a bien diagnostiqué

### Triple fault → page fault dans buddy allocator sous CR3 userspace

**Observation Codex** :
> *"La faute est dans le buddy allocator pendant fork : sous CR3 userspace, le noyau essaie d'accéder à ses métadonnées physiques basses autour de 0x06fd1ef8"*

**Évaluation** : Correct. Le kernel, après le fork, tente d'accéder aux structures de l'allocateur physique via des adresses virtuelles dans la fenêtre identité basse, qui n'est pas mappée dans le CR3 userspace. La correction (mapper toute la région `[1MiB, __kernel_end]` dans les PML4 userspace) est valide et nécessaire.

### Panic sur `extend_from_slice` dans le clone FD

**Observation Codex** :
> *"fork passe le clonage CoW, crée le thread fils, puis panique dans le clonage de la table des descripteurs"*

**Évaluation** : Correct. La méthode originale `extend_from_slice` sur un vecteur no_std en contexte noyau peut paniquer si la réservation échoue silencieusement. La correction par copie explicite fallible est la bonne approche.

### SYSRET atteint mais PID1 silencieux

**Observation Codex** :
> *"Le retour SYSRET est atteint, mais PID1 ne logue plus après fork. Ça pointe très fort vers le premier write userspace après retour, donc vers la faute Copy-on-Write de la stack parent."*

**Évaluation** : Partiellement correct sur le symptôme, mais la cause racine est différente.

---

## Où Codex s'est trompé ou s'est arrêté trop tôt

### Erreur d'attribution : "CoW de la stack parent"

Codex attribue le silence de PID1 après fork à un échec CoW sur la pile du **parent**. Or :

1. Le **parent** (PID1 = init_server) a un `VmaTree` intact. Son CoW break sur la pile devrait fonctionner une fois BUG-C1 corrigé.

2. La vraie cause du silence est **BUG-C1** : `KernelFaultAllocator` opère sur `KERNEL_AS.pml4_phys()` au lieu du CR3 courant. Quand le parent touche sa pile (CoW), le handler lit le PTE dans le PML4 noyau (qui n'a pas les mappings user) → retourne 0 → fallback demand_paging → essaie de mapper dans KERNEL_AS → échec silencieux. Le parent ne reçoit jamais la correction de sa page table → #PF infini (ou SIGSEGV si le handler envoie le signal, puis reboucle via le scheduler).

3. Même si BUG-C1 était corrigé, **BUG-C2** (VmaTree vide du fils) tuerait tous les services dès leur premier accès mémoire.

### Correction incomplète du mapping noyau bas

Codex a ajouté des mappings `[1MiB, __kernel_end]` dans les CR3 userspace. C'est une amélioration nécessaire mais insuffisante. Le problème fondamental est que `KernelFaultAllocator` n'utilise jamais le bon CR3 pour les PTEs utilisateur — ajouter des sections kernel dans le CR3 user ne résout pas ça.

### Axe IPC/TTY non investigué

Codex s'est concentré sur le chemin kernel (fork, CoW, ELF loader) sans examiner la chaîne TTY → stdin/stdout → shell. Les BUG-C3 et BUG-C4 (FDs jamais ouverts, pas de pont TTY↔FD) n'ont pas été touchés. Même si fork et exec fonctionnaient parfaitement, le shell resterait silencieux et aveugle.

---

## Timeline reconstructée du blocage

```
1. [✓ RÉSOLU par Codex] Triple fault : kernel_low non mappé dans CR3 user
   → Correction : map [1MiB, __kernel_end] dans chaque PML4 user

2. [✓ RÉSOLU par Codex] Panic slice dans clone FD
   → Correction : copie fallible des FDs

3. [✗ NON RÉSOLU] SYSRET atteint, mais PID1 silencieux après fork
   → Vrai cause : BUG-C1 (KernelFaultAllocator → KERNEL_AS, pas CR3 courant)
   → Conséquence : tout CoW/demand-paging userspace est inopérant

4. [✗ NON RÉSOLU, non investigué] Fils (ipc_router, etc.) SEGFAULT immédiat
   → Cause : BUG-C2 (VmaTree vide pour tout processus fils)

5. [✗ NON RÉSOLU, non investigué] Shell interactif impossible même si boot réussit
   → Cause : BUG-C3 (FDs 0/1/2 vides) + BUG-C4 (aucun pont TTY)
```

---

## Recommandation pour la suite

L'ordre optimal de corrections :

```
Phase 1 (débloquer le boot) :
  PATCH-C1 → KernelFaultAllocator utilise current_pml4_phys()
  PATCH-C2 → VmaTree cloné dans fork_impl.rs
  PATCH-M1 → DEPS_SCHEDULER corrigé

Phase 2 (débloquer le shell) :
  PATCH-C3 → install_std_fds pour init
  PATCH-C4-A → exosh via IPC TTY

Phase 3 (robustesse) :
  BUG-M1 → readiness IPC réelle
  BUG-M3 → clarifier quel exosh est embarqué
  BUG-M4 → vérifier SWAPGS dans fork_child_trampoline
```

---

## Note sur la méthode de Codex

La méthode "boot QEMU + lire les logs E9/serial + inférer la cause" est efficace pour les crashes visibles (triple fault, panic avec message). Elle est insuffisante pour les bugs silencieux (handler retourne Handled sans avoir rien fait → le processus ne crashe pas, il tourne mais ne progresse pas). BUG-C1 est exactement ce type de bug silencieux : le CoW handler retourne `Handled` (via le fallback demand_paging → map dans KERNEL_AS → "succès"), mais le PTE user n'a jamais été modifié. Le processus retente le même accès → même #PF → boucle infinie ou scheduler timeout.

Une analyse statique du code (comme celle de ce rapport) est complémentaire indispensable pour détecter ce type de bug.
