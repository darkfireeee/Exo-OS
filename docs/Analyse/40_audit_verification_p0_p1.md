# 40 — Rapport de Contre-Vérification et Corrections d'Audit (P0 & P1)

Ce rapport documente la contre-vérification rigoureuse, fichier par fichier et ligne par ligne, du rapport d'audit externe (`Analyse_z-ai`), ainsi que l'ensemble des correctifs de sécurité et de robustesse appliqués au projet **Exo-OS**.

---

## 1. Synthèse des Vulnérabilités et Anomalies P0 Vérifiées et Corrigées

### T6-001 / T9-006 — Élévation de privilèges Ring 0 / Ring 3 via `rt_sigreturn`
- **Fichier** : `kernel/src/syscall/dispatch/signals.rs` et `kernel/src/process/signal/handler.rs`
- **Constat** : `sys_rt_sigreturn` ne masquait que le Trap Flag (`regs.rflags & !0x100`) avant de restaurer les registres via SYSRETQ. Un programme en CPL 3 pouvait forger un UContext avec `IOPL=3` ou désactiver `IF=0`, lui accordant les instructions d'E/S directes (`in`/`out`/`cli`/`sti`) sans passer par le noyau.
- **Correction** : Application stricte de `RFLAGS_SAFE_MASK = 0x0024_0CD5`, `RFLAGS_FORCE_SET = 0x0200` (IF=1), et `RFLAGS_FORCE_CLR = 0x0003_7100` (IOPL=0, TF=0, NT=0, RF=0, VM=0).

### T9-024 — Panique #GP(0) noyau via `sys_arch_prctl`
- **Fichier** : `kernel/src/syscall/handlers/misc.rs`
- **Constat** : `ARCH_SET_FS` et `ARCH_SET_GS` écrivaient directement l'adresse utilisateur dans `MSR_FS_BASE` et `MSR_KERNEL_GS_BASE`. Sur x86_64, toute adresse non-canonique transmise à `WRMSR` provoque immédiatement une `#GP(0)` en Ring 0 (crash instantané du noyau).
- **Correction** : Validation obligatoire via `is_user_canonical(addr)` avant toute écriture MSR, et mise à jour synchronisée du TCB du thread courant.

### T9-007 — Panique Page Fault Ring 0 via `sys_uname`
- **Fichier** : `kernel/src/syscall/handlers/misc.rs`
- **Constat** : `sys_uname` écrivait directement dans la mémoire utilisateur via `core::ptr::write_bytes` sans vérifier la validité de l'adresse ni intercepter les défauts de page.
- **Correction** : Formatage de la structure `UtsName` sur la pile noyau, suivi d'une écriture sécurisée via `write_user_typed` avec fixup de défaut de page.

### T9-004 / T4-003 — Déréférencement mémoire non validé dans `futex`
- **Fichier** : `kernel/src/syscall/table/time.rs` et `kernel/src/memory/utils/futex_table.rs`
- **Constat** : `sys_futex` et `futex_wait` exécutaient `(virt_addr as *const u32).read_volatile()` sans vérifier l'alignement sur 4 octets, la limite `USER_ADDR_MAX` ou la canonicité de l'adresse.
- **Correction** : Validation stricte de `uaddr % 4 == 0`, `uaddr < USER_ADDR_MAX`, `is_user_canonical`, et protection de lecture volatile.

### T9-001 — Bris d'isolation des descripteurs de fichiers (`resolve_fd`)
- **Fichier** : `kernel/src/syscall/fs_bridge/fd.rs`
- **Constat** : En cas d'absence d'un `fd > 2` dans la table du processus appelant, `resolve_fd` retournait un `ResolvedFd` avec `handle: fd`, permettant à un processus arbitraire d'accéder aux objets globaux de n'importe quel autre processus.
- **Correction** : Tout descripteur absent de `process_fd_descriptor` retourne désormais strictement `Err(FsBridgeError::BadFd)`.

### T9-003 — Usurpation de gestion de mémoire distante
- **Fichier** : `kernel/src/syscall/table/ipc/remote_memory.rs`
- **Constat** : `caller_can_manage_target_memory` et `caller_can_copy_target_memory` vérifiaient uniquement une chaîne de caractères mutable `"memory_server"`.
- **Correction** : Durcissement exigeant que l'appelant soit un descendant direct d'init (`ppid == 1`), root (`euid == 0`), et authentifié.

### T1-025 — Échec en cascade #PF récursif dans le dump de fautes CPU
- **Fichier** : `kernel/src/arch/x86_64/exceptions/frame.rs`
- **Constat** : `dump_cpu_fault_frame` tentait de lire les adresses `rip`, `rsp` et `rdi` utilisateur avec un simple `read_volatile()`. Si l'exception provenait d'un saut ou d'une pile non mappée, cette lecture provoquait un second `#PF` en Ring 0, menant à un `#DF` (Double Fault) et l'arrêt du CPU.
- **Correction** : Remplacement des déréférencements bruts par `crate::syscall::fixup::copy_from_user_safe`.

### T4-002 — Borne haute manquante dans `mmap(MAP_FIXED)`
- **Fichier** : `kernel/src/memory/virtual/mmap.rs`
- **Constat** : Le chemin `MAP_FIXED` n'interdisait pas les adresses virtuelles `>= USER_END`, permettant à userspace d'écraser les structures d'espace d'adressage du noyau.
- **Correction** : Rejet de tout mapping dont la base est `< PAGE_SIZE` ou dont la fin dépasse `USER_END`.

### T4-001 — Libération erronée de pages physiques partagées dans `munmap`
- **Fichier** : `kernel/src/memory/virtual/mmap.rs`
- **Constat** : `do_munmap_in_as` appelait inconditionnellement `buddy::free_page()` sur les cadres physiques, même pour les fichiers projetés, SHM, ou régions MMIO.
- **Correction** : Filtrage strict vérifiant que le backing est `VmaBacking::Anonymous` avant de libérer le cadre physique au buddy allocator.

### T7-002 — Panique #GP(0) matériel réel via `XRSTOR`
- **Fichier** : `kernel/src/scheduler/fpu/save_restore.rs`
- **Constat** : `arch_xrstor64` passait `!0u64` comme masque RFBM. Sur processeur physique réel, si un bit de RFBM n'est pas activé dans `XCR0`, `XRSTOR` lève immédiatement une `#GP(0)`.
- **Correction** : Remplacement du masque `!0u64` par `read_xcr0()` dynamique.

### T8-001 — Fuite de droit d'écriture en mémoire partagée
- **Fichier** : `kernel/src/ipc/shared_memory/page.rs` et `mapping.rs`
- **Constat** : `PageFlags::SHM_DEFAULT` contenait `WRITE` par défaut, rendant accessibles en écriture des mappings demandés en lecture seule.
- **Correction** : Suppression du flag `WRITE` lorsque `!region_perms.can_write()`.

### T5-001 / T5-002 — Alignement SLUB et initialisation à zéro
- **Fichier** : `kernel/src/memory/heap/allocator/hybrid.rs` et `physical/allocator/slub.rs`
- **Constat** : `hybrid::alloc` ignorait les alignements supérieurs à 64 octets, et `SlubCache::alloc` ne respectait pas le flag `AllocFlags::ZEROED`.
- **Correction** : Routage des alignements `> 64` vers `vmalloc::kalloc` (aligné sur 4096 octets), et mise à zéro explicite de la mémoire lorsque `AllocFlags::ZEROED` est positionné.

### T11-001 — Révocation non autorisée de capability
- **Fichier** : `kernel/src/security/capability/mod.rs` et `kernel/src/syscall/table/ipc/security_ops.rs`
- **Constat** : `sys_exo_cap_revoke` permettait à n'importe quel processus de révoquer n'importe quel handle sans vérifier l'appartenance de la capability.
- **Correction** : Ajout de `revoke_handle_by_caller` restreignant la révocation au propriétaire (`meta.owner_pid == caller_pid`) ou à la cible désignée.

### T6-002 / T6-003 — Destruction indue des ressources de processus lors de l'arrêt d'un thread
- **Fichier** : `kernel/src/process/lifecycle/exit.rs`
- **Constat** : `mark_exit` fermait les fichiers, le VFS et les drivers du processus dès qu'un simple thread secondaire appelait `sys_exit`.
- **Correction** : Exécution du nettoyage global uniquement lorsque le compteur de threads restants atteint 0 (`remaining_threads == 0`).

### T14-001 & T14-002 & T14-003 — Paniques et corruption dans ExoFS (`ConfigEntry` & `RelationEntry`)
- **Fichier** : `kernel/src/fs/exofs/objects/object_kind/config.rs` et `relation.rs`
- **Constat** : `from_disk` ne vérifiait pas `key_len <= 64`, `val_len <= 256` ni `label_len <= 32`. L'appel ultérieur à `key_bytes()`, `value_bytes()` ou `label_bytes()` provoquait un dépassement de tampon (`panic!`). De plus, `ConfigStore::set` n'empêchait pas l'écrasement avec une valeur `> 256` octets.
- **Correction** : Bornage strict dans `verify()` et `from_disk()`, rejet préalable dans `ConfigStore::set`.

### T17-001 — Implémentation altérée de Blake3 dans la déduplication ExoFS
- **Fichier** : `kernel/src/fs/exofs/dedup/chunk_fingerprint.rs`
- **Constat** : `blake3_hash` utilisait une fonction maison à 1 seul round sans permutations, causant de fausses collisions et des corruptions de déduplication.
- **Correction** : Remplacement direct par l'implémentation officielle standardisée `crate::fs::exofs::core::blake3_hash`.

### T18-001 — Absence de validation des plages mémoire userspace dans ExoFS
- **Fichier** : `kernel/src/fs/exofs/syscall/validation.rs`
- **Constat** : `copy_from_user` et `copy_to_user` ne validaient pas la borne haute `USER_ADDR_MAX`, permettant à des requêtes userspace de cibler la mémoire noyau.
- **Correction** : Validation stricte des adresses et des longueurs (`addr + len <= USER_ADDR_MAX`).

### T16-001 — Écrasement destructif du superbloc primaire par fsck
- **Fichier** : `kernel/src/fs/exofs/recovery/fsck.rs` et `fsck_repair.rs`
- **Constat** : En cas de simple divergence de checksum, `fsck` écrasait le superbloc primaire avec un bloc de repli vide, détruisant tous les pointeurs de la partition.
- **Correction** : Interdiction absolue d'écraser le superbloc primaire avec un template vide ; préservation des données pour restauration par miroir.

### T17-002 — Risque d'éradication massive par le ramasse-miettes d'orphelins
- **Fichier** : `kernel/src/fs/exofs/gc/orphan_collector.rs`
- **Constat** : Si le snapshot ne découvrait aucune racine alors que des objets étaient enregistrés, tous les objets vivants étaient identifiés comme orphelins et marqués pour suppression.
- **Correction** : Interruption sécurisée de la passe si aucune racine n'est atteignable alors que des objets existent.

### T18-002 — Double allocation et corruption de blocs dans `reserve_for_write`
- **Fichier** : `kernel/src/fs/exofs/syscall/object_store.rs`
- **Constat** : L'ancien extent était libéré dans la liste des blocs libres *avant* de vérifier si la nouvelle allocation réussissait, générant un état incohérent en cas de `NoSpace`.
- **Correction** : Libération de l'ancien extent uniquement après le succès confirmé de la nouvelle allocation.

### T20-001 — Usurpation de la réinitialisation Phoenix du CSPRNG
- **Fichier** : `servers/crypto_server/src/dispatch.rs`
- **Constat** : `PHOENIX_WAKE_ENTROPY` se fiait uniquement à `req.sender_pid == 0`, falsifiable par tout processus n'utilisant pas l'injection sécurisée de PID par le noyau.
- **Correction** : Validation obligatoire via `exo_cap_check` exigeant une capability kernel authentique avant d'autoriser la commande.

### T13-001 / T13-002 — Écrasement physique de la mémoire basse BIOS (0x0)
- **Fichier** : `exo-boot/src/kernel_loader/mod.rs` et `exo-boot/src/main.rs`
- **Constat** : Pour un noyau non-PIE lié à 1 MiB, `load_kernel(&params, 0)` utilisait `phys_base = 0`, écrasant la table des vecteurs d'interruption (IVT) et la BDA à l'adresse physique 0x0.
- **Correction** : Enregistrement de l'adresse par défaut à `0x100_000` (1 MiB) en l'absence de KASLR/PIE, et appel explicite avec `0x100_000`.

### T12-001 — Primitive de lecture/écriture physique arbitraire dans les serveurs bloc
- **Fichier** : `drivers/storage/common_hal/src/block_ipc.rs`
- **Constat** : `run_block_server` déréférençait `cmd.dma_phys` fourni par le client IPC sans contrôle de plage, permettant d'écraser la mémoire d'autres processus.
- **Correction** : Validation stricte exigeant que l'adresse appartienne à l'espace utilisateur (`addr + 4096 <= USER_SPACE_MAX`), rejetant toute tentative illégitime avec `EFAULT`.

### T12-002 — Paramètres DMA/IOVA non vérifiés dans les drivers réseau
- **Fichier** : `drivers/network/e1000/src/main.rs` et `drivers/network/virtio_net/src/main.rs`
- **Constat** : `apply_driver_init` acceptait des adresses `rx_base_iova` et `tx_base_iova` nulles ou non alignées sans contrôle.
- **Correction** : Validation de l'alignement sur page (`% PAGE_SIZE == 0`), non-nullité et conformité de la taille du pool.

---

## 2. Synthèse des Correctifs P1 Appliqués

1. **T1-021** : `kernel/src/arch/x86_64/syscall.rs` — Écriture du MSR CSTAR conditionnée au vendor CPU AMD (`cpu_features().vendor == CpuVendor::Amd`), évitant le `#GP(0)` au boot sur processeurs Intel réels.
2. **T1-024** : `kernel/src/arch/x86_64/exceptions/frame.rs` — Suppression de la définition dupliquée de `USER_PF_DEBUG_COUNT` (résolution de l'erreur `E0428`).
3. **T1-029** : `kernel/src/arch/x86_64/exceptions/cpu_faults.rs` — Envoi systématique du signal POSIX approprié (`SIGFPE`, `SIGSEGV`, `SIGBUS`) via `queue_signal_for_current` avant `exception_return_to_user` sur `#DE`, `#OF`, `#BR`, `#NP`, `#SS`, `#MF`, `#XM`, `#AC`, `#VE`, éliminant le livelock CPU à 100% lors des exceptions arithmétiques ou mémoire en Ring 3.
4. **T10-001** : `kernel/src/drivers/pci_cfg.rs` — Bornage strict de l'offset de configuration PCI (`offset <= 0xFC` et alignement sur 4 octets) avec masquage `& 0xFC`, empêchant le débordement de l'offset dans les champs dev/func/bus.
5. **T10-002** : `kernel/src/drivers/dma.rs` — Ajout du TLB shootdown multi-cœur synchrone (`shootdown_sync`) lors de la libération des allocations DMA (`unmap_dma_alloc_record`), évitant les accès concurrents post-libération depuis les cœurs secondaires.
6. **T1-050** : `kernel/src/arch/x86_64/framebuffer_early.rs` — Vérification que `size_bytes >= stride * height * bpp` et `stride >= width` pour éliminer tout risque d'écriture hors limites lors de l'affichage vidéo initial.
