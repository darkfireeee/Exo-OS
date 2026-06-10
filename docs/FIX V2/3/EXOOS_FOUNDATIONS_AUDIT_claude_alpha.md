# ExoOS — Audit des Fondations Architecturales
**Sources :** claude-alpha (lecture code), claude-beta (analyse structurelle), claude-delta (analyse systèmes)  
**Méthode :** claude-alpha = lecture directe du code (vérité terrain). beta/delta = analyse architecturale. Conflits résolus par le code.

---

## Corrections inter-analyses

Avant la synthèse, les contradictions entre les trois analyses, tranchées par le code réel :

| Claim | Source | Verdict code |
|-------|--------|-------------|
| KASLR présent | delta | **Faux** — `KERNEL_START = 0xFFFF_FFFF_8000_0000` hardcodé dans `layout.rs` |
| `ready_mask: u64` → limite 64 services | beta | **Partiellement faux** — `SERVICES: [Service; 15]` utilise `AtomicBool` par service, pas de bitmask stocké. MAIS `runtime_running_mask() -> u64` dans `service_table.rs` retourne bien un u64. Contrainte latente, pas structurelle |
| Payloads `include_bytes!` = béquille permanente | delta | **Exagéré** — mécanisme conditionnel (`EXO_BOOT_PAYLOAD_DIR`). La production charge depuis ExoFS/BLOB_CACHE via `resolve_path_to_blob`. Build debug seulement |
| USB complètement absent | beta | **Partiellement faux** — `drivers/input/usb_hid/src/` existe, répertoire vide. Squelette présent, zéro implémentation |
| Display = uniquement early boot | beta | **Partiellement faux** — `drivers/display/framebuffer/` a un vrai driver (PixelFormat, FramebufferInfo, blit, cursor). Pas DRM/KMS mais pas rien |
| DNS absent | beta | **Confirmé** — `dns_guard.rs` dans exo_shield est un filtre sécurité, pas un resolver |

---

## Fondations solides — construire dessus sans risque

### Mémoire (couche 0)
Buddy NUMA-aware, slab/slub, SLUB thread-local, CoW tracker, VMA tree avec opérations (split/merge/walk), demand paging via `FileFaultProvider` injectable, frame descriptors LRU, reclaim CLOCK, guard pages, KPTI dual-PML4, huge pages (hugetlbfs + THP), vmalloc, DMA channels avec affinité/priorité. **Aucune réécriture à prévoir.** Tout est extensible par composition.

### Scheduler (couche 1, Ring0 pur)
Fast-path ASM (`fast_path.s` + `switch_asm.s`), CFS + deadline + realtime + idle, SMP load balancing, IPI de reschedule, hrtimer + tick + deadline timers, FPU lazy save/restore, energy/C-states. `scheduler_server` Ring1 = politique uniquement (nice, affinité, admission RT). **Architecture microkernel correcte.**

### Sécurité hardware
KPTI (dual PML4 par CPU), retpoline (`__x86_indirect_thunk_rax/r11` + macros), IBRS + STIBP + IBPB, SSBD, SMEP + SMAP (détectés CPUID, appliqués CR4), CET Shadow Stack (détecté), PKU (détecté). Appliqués sur BSP et chaque AP au boot. **Suite complète.**

### Modèle processus
PCB complet (pid/ppid/tgid/sid/pgid, creds, files, address_space, brk, signaux, namespaces), fork/clone (CLONE_FILES, CLONE_VM, CLONE_SIGHAND, CLONE_THREAD, vfork), exec/exit/wait/reap, signals (delivery + handler + queue + mask), session + pgrp + job_control, futex (SipHash keyé, injection réveil par fn pointer), AUXV complet (AT_PHDR, AT_BASE, AT_ENTRY, TLS, AT_SIGNAL_TCB). **Base pthread_create disponible.**

### ABI syscall
Numéros 0–299 calqués sur Linux x86_64, extensions 300+ pour ExoOS. musl/glibc pourra tourner sans patch sur les syscalls implémentés. Layering documenté (memory couche 0 → scheduler 1 → IPC 2 → FS 3) et respecté dans le code. **Fondation de compatibilité solide.**

### IPC kernel
Endpoints, channels SPSC/MPMC, RPC avec timeout + flags, ring buffers, IPC_FLAG_TIMEOUT encodage correct. `scheduler_server`, `memory_server`, `device_server` : modèle de serveurs Ring1 cohérent. **Extensible.**

### ExoFS
5 couches de cache (blob, object, extent, metadata, path), eviction ARC, shrinker, pressure monitor, cache_warming. Epochs, snapshots, dedup, crypto par objet, GC tricolore, NUMA-aware, compression. OOM killer présent. **Surpuissant pour le stade actuel.**

### epoll / multiplexage I/O
`sys_epoll_create/ctl/wait/pwait/pwait2`, `sys_poll`, `sys_select`, `sys_ppoll`, `sys_eventfd/2`. Tous implémentés dans `table.rs`. **Serveurs réseau à event-loop possibles.**

### Namespaces
PID, Mount, Net, UTS, User — 5 sur 6. `NsSet` dans PCB. Fondation containerisation présente.

---

## Fondations posées mais incomplètes — remplissage nécessaire, pas de réécriture

### mmap file-backed
`VmaBacking::File` existe dans `VmaDescriptor`, `FileFaultProvider` est un trait injectable, le fault handler a le chemin `VmaBacking::File =>` câblé. **Ce qui manque :** `do_mmap` ignore le `_fd` (commentaire littéral dans le code). Connecter fd → VMA File-backed = ~200 lignes, extension propre du chemin existant.

### Partage physique des pages ELF entre processus
`FileFaultProvider::load_file_page` fait `alloc_nonzeroed() + copy_from_slice` à chaque fault. 10 processus = 10 copies physiques du même `.text`. **Ce qui manque :** une table globale `(inode_id, file_offset) → Frame` avec ref-counting. Le trait `FileFaultProvider` est déjà injectable — le remplacer par une implémentation shared-frame est transparent pour le reste du kernel.

### Shared libraries
`PT_DYNAMIC`, `DT_RELA`, `JMPREL`, `init_array` : parsés et appliqués. Les fichiers `library.rs`, `resolver.rs`, `symbol_table.rs`, `search_path.rs` existent dans le loader. **Ce qui manque :** résolution `DT_NEEDED` → chargement `.so` → relocation inter-objets. `UnsupportedNeededLibrary` est retourné aujourd'hui. Gros travail (c'est tout un linker dynamique), mais la structure d'accueil est là.

### Swap actif
`SwapBackendRegistry`, `SwapDevice` trait, `kswapd_reclaim()` CLOCK, `try_swap_out()` — infrastructure complète. **Ce qui manque :** un driver concret enregistrant `&'static dyn SwapDevice` au boot. Un driver zram ou virtio-blk-swap suffit pour activer le système.

### chdir / getcwd
Les syscalls `SYS_CHDIR` et `SYS_GETCWD` sont câblés dans `table.rs`. **Ce qui manque :** `fs_chdir` valide le répertoire mais ignore le pid et ne stocke rien. `fs_getcwd` retourne `"/"` hardcodé. PCB a `files: SpinLock<OpenFileTable>` — la pattern d'ajout d'un champ par processus est établie. Ajouter `cwd: SpinLock<[u8; PATH_MAX]>` dans PCB + câbler les deux fonctions = ~50 lignes.

### KASLR
`KERNEL_START` hardcodé. UEFI fournit `EFI_RNG_PROTOCOL`. Appliquer un slide aléatoire avant la construction des page tables au boot, sans toucher au reste du layout. Extension de boot, pas de rearchitecture.

### Cache ExoFS ↔ reclaim physique
`CacheShrinker` implémente un trait `shrink()`. `kswapd_reclaim` libère des frames via CLOCK. **Ce qui manque :** `kswapd_reclaim` n'appelle pas `CACHE_SHRINKER.shrink()`. Le point de couplage est évident, c'est une ligne d'appel.

### io_uring userspace
`IoUringSqe/IoUringCqe`, ring SQ/CQ, submit/reap existent dans `fs/exofs/io/io_uring.rs`. **Ce qui manque :** exposition comme `SYS_IO_URING_SETUP/ENTER/REGISTER` dans la table de syscalls. Les types existent, il faut les rendre accessibles depuis le userspace.

---

## Fondations absentes — conception nouvelle requise

### Page cache unifié
Aujourd'hui : blob_cache (ExoFS) et frames physiques (LRU reclaim) sont deux mondes séparés. `mmap`, `read()`, et demand paging ne partagent pas les mêmes frames pour le même fichier. Pour y arriver : un objet central `PageCache<inode_id>` faisant autorité sur `(inode, offset) → Frame`. C'est une conception nouvelle qui touche à la fois à `memory/virtual/fault/` et à `fs/exofs/cache/`. **La fondation mémoire et FS est prête pour le recevoir, mais il n'existe pas encore.**

### Lazy binding PLT
Résolution JMPREL au load-time avec `NoSymbols`. Pour le lazy binding : stubs PLT + `dl_resolve` appelé au premier appel + remplissage GOT à la demande. Nécessite une trampoline dans le loader userspace et un protocole kernel↔loader. **Conception nouvelle, mais décidable indépendamment du reste.**

### Namespace réseau fonctionnel
`net_ns` est présent dans `NsSet` (PCB) et `NetNamespace` dans `net_ns.rs`. **Mais** `network_server` est un singleton global — il n'y a pas de binding entre namespace processus et instance réseau isolée. Pour des conteneurs avec isolation réseau réelle, `network_server` doit être instanciable par namespace. Architecture Ring1 multi-instances à concevoir.

### Hotplug / notification udev-like
`smp/hotplug.rs` gère les CPUs. `device_server` a un `hotplug.rs`. Mais il n'existe pas de bus de notification standardisé (type netlink/uevent) vers les applications. Pour USB branchable, disques amovibles, cartes réseau : fondation à concevoir.

### ABI IPC stable entre kernel et serveurs
Les structs `NetMsg/NetReply`, opcodes, structures IPC sont versionnées ponctuellement mais sans schéma de compatibilité forward/backward. Quand les serveurs Ring1 évolueront indépendamment du kernel (c'est l'intention d'un microkernel), les mises à jour partielles casseront sans discipline formelle ici. Un mécanisme de capability negotiation ou de version handshake à la connexion endpoint est à concevoir.

### DNS resolver
`dns_guard.rs` dans exo_shield = filtre sécurité uniquement, pas un resolver. Aucun équivalent de `libresolv`, pas de `/etc/resolv.conf`, pas de cache DNS. Pour un OS "connecté au monde", c'est une fondation de même rang que TCP. Peut vivre en userspace, mais son ABI syscall (getaddrinfo-like) doit être pensée.

---

## Tableau de synthèse

| Composant | État | Effort pour compléter |
|-----------|------|-----------------------|
| Mémoire bas niveau | ✅ Solide | — |
| Scheduler Ring0 | ✅ Solide | — |
| Sécurité CPU | ✅ Solide | — |
| Modèle processus | ✅ Solide | — |
| ABI syscall Linux-compat | ✅ Solide | — |
| IPC kernel | ✅ Solide | — |
| ExoFS + cache | ✅ Solide | — |
| epoll/poll/select | ✅ Solide | — |
| mmap file-backed | 🔶 Fondation présente | ~200 lignes |
| Partage pages ELF | 🔶 Fondation présente | Nouvelle impl FileFaultProvider |
| chdir/getcwd | 🔶 Fondation présente | ~50 lignes + champ PCB |
| KASLR | 🔶 Fondation présente | Extension boot UEFI RNG |
| Swap actif | 🔶 Fondation présente | Driver zram/virtio-swap |
| Cache ↔ reclaim | 🔶 Fondation présente | Couplage 1 appel |
| io_uring userspace | 🔶 Fondation présente | Exposition syscall |
| Shared libraries | 🔶 Structure accueil présente | Gros travail (linker dynamique) |
| Page cache unifié | 🔴 Conception nouvelle | Design cross-mm/fs |
| Lazy binding PLT | 🔴 Conception nouvelle | Protocole loader↔kernel |
| Namespace réseau multi-instance | 🔴 Conception nouvelle | Ring1 multi-instanciable |
| Hotplug / udev-like | 🔴 Conception nouvelle | Bus de notification |
| ABI IPC stable | 🔴 Conception nouvelle | Protocol négociation version |
| DNS resolver | 🔴 Conception nouvelle | Userspace service + ABI |
| USB HID | ⚠️ Squelette vide | Implémentation XHCI + HID |
| Display DRM/KMS | ⚠️ Framebuffer présent | Abstraction driver display |
| IPC namespace (SysV) | ⚠️ Absent de NsSet | Ajout champ + module |

---

## Lecture globale

Le kernel a une base intellectuellement solide. Les fondations basses (mémoire, scheduler, sécurité, processus, ABI) sont de qualité. La plupart des manques dans la colonne 🔶 sont du remplissage — la structure est là, le corps manque — et peuvent être traités indépendamment dans n'importe quel ordre.

Les manques 🔴 sont des conceptions nouvelles mais elles ne remettent pas en cause ce qui est posé. Elles se greffent dessus.

Le seul vrai verrou de chaîne est : **shared libraries → mmap file-backed → page cache unifié**. Ces trois points sont dépendants l'un de l'autre dans l'ordre. Sans shared libs, aucun binaire standard ne tourne. Sans mmap file-backed, les shared libs ne peuvent pas être chargées efficacement. Sans page cache unifié, le partage physique des pages de code entre processus est impossible. C'est le chantier principal.
