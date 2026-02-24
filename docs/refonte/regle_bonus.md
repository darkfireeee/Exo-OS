═══ LOCK ORDERING (RÈGLE ABSOLUE) ═══
Pour éviter les Deadlocks, l'ordre de prise de verrous est STRICT :
1.  IPC Locks (Plus bas niveau)
2.  Scheduler / Thread Locks
3.  Memory / PageTable Locks
4.  FS / Inode Locks (Plus haut niveau)
⛔ INTERDICTION FORMELLE de prendre un lock de niveau N si on possède déjà un lock de niveau N+1.


═══ ZONES "NO-ALLOC" ═══
🚫 Interdiction d'utiliser `alloc`, `Vec`, `Box`, `Rc` ou `Arc` dans :
1.  Tout fichier dans `scheduler/core/`
2.  Les handlers d'interruption (ISR)
3.  Le code appelé quand `preemption_disable()` est actif.
✅ Utiliser uniquement : Stack, Static buffers, ou `ring::slot`.


═══ CONTRAT UNSAFE ═══
Tout bloc `unsafe { ... }` DOIT être précédé d'un commentaire `// SAFETY: ...` expliquant pourquoi c'est sûr.
Si l'IA génère du `unsafe` sans justification : REJET AUTOMATIQUE.