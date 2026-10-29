# RAPPORT D'AUDIT APPROFONDI (SESSION 2H) : IPC, FS ET SYSCALL

**Date :** 29 Octobre 2026  
**Cible :** `x86_64-unknown-none` (Micro-Noyau Exo-OS Ring 0 & Pilotes Ring 1)  
**Sous-systèmes audités :** IPC, Système de Fichiers (ExoFS, Ext4, FAT32, VFS), Appels Système (Syscalls)

---

## 1. Synthèse Exécutive

Conformément à la directive d'audit approfondi de 2 heures centré exclusivement sur l'**IPC**, le **FS** et les **SYSCALLS**, les rapports de signalements des trois dossiers de référence (`Analyse_2`, `Analyse_z-ai`, `Analyse (3)`) ont été passés au crible et confrontés au code source.

- **11 nouvelles failles réelles et critiques** ont été identifiées, contre-vérifiées et corrigées dans le code source :
  1. **IPC Endpoints (P0 - Squatting & Impersonation)** : `RegistryInner::insert` écrasait aveuglément l'EndpointId lors d'une collision de hash (`slot.hash == cur_hash`), permettant à un processus malveillant de détourner les noms de services critiques ("memory_server", "vfs_server", "exo_shield"). Désormais, toute tentative d'écrasement par un endpoint tiers est formellement rejetée avec `Err(IpcError::AlreadyConnected)`.
  2. **IPC Channel (P1 - Uncancellable Recv DoS)** : Dans `recv_ipc_message`, la boucle de réception bloquante ne testait pas les signaux en attente avant de s'endormir. Un signal reçu (ex. `SIGKILL`, `SIGINT`) pouvait laisser le thread piégé en kernel. L'interrogation systématique de `has_signal_pending()` avant chaque tentative garantit la sortie immédiate avec `-EINTR`.
  3. **IPC System Ops (P2 - Kernel Log Flooding)** : `sys_exo_log` n'appliquait aucun contrôle d'accès, permettant à tout binaire non privilégié d'inonder la console kernel avec 4 Ko par appel. L'accès est désormais réservé aux processus root (EUID=0) ou aux services enregistrés (`ServiceClass`).
  4. **IPC SPSC Ring (P2 - Accurate Monotonic Timeout)** : Dans `spsc_wait_reply`, le calcul de timeout reposait sur un compteur d'itérations de boucle empirique (`spins > timeout_ns / 100`). Il est désormais indexé sur l'horloge monotone réelle (`monotonic_ns`).
  5. **Syscall Readv (P2 - Truncated Socket Read)** : Dans `sys_readv`, la branche socket retournait immédiatement dès le premier buffer non vide, ignorant tous les iovecs suivants. Les lectures sont désormais accumulées sur l'ensemble des tranches avec coupure propre sur lecture partielle, en parfaite parité avec `sys_writev`.
  6. **Syscall Mremap (P2 - Virtual Address Leak)** : Lors d'un rétrécissement (`new_len < old_len`), `sys_mremap` retournait l'ancienne adresse sans désallouer le surplus, créant une fuite silencieuse d'espace virtuel et physique. Les pages excédentaires sont maintenant démappées via `do_munmap`.
  7. **Syscall Mmap (P3 - Integer Overflow)** : Sécurisation du calcul de pages `len.saturating_add(4095) / 4096` contre les débordements `usize`.
  8. **FS ExoFS Storage (P1 - Deadlock VirtIO Global Disk)** : `with_global_disk` maintenait le verrou spinlock `GLOBAL_DISK` pendant toute la durée des entrées/sorties physiques synchrones. L'extraction par clone d'une référence `Arc<dyn BlockDevice>` libère le verrou avant le déclenchement de l'I/O, éliminant les contentions et inversions de verrouillage lors des flushes.
  9. **FS ExoFS Storage (P2 - MMIO Page Leak)** : En cas d'échec partiel de mapping virtuel dans `kernel_mmio_phys_to_virt`, les pages déjà mappées sont désormais démappées en rollback (`KERNEL_AS.unmap`).
  10. **FS ExoFS GC (P1 - Garbage Collector Reclaim Failure)** : `sweeper.rs` extrayait les blobs de la `DeferredDeleteQueue` sans jamais appeler `BLOB_REFCOUNT.remove_zero(&entry.blob_id)`. Les blobs orphelins demeuraient indéfiniment en table avec un refcount à 0. L'appel effectif de suppression et de mise à jour des métriques a été restauré.
  11. **FS Drivers Ext4 (P1 - Out-of-Bounds Extent Read)** : Dans `drivers/fs/src/ext4/extent.rs`, `find_extent` lisait `eh_entries` sans borner la taille du tampon `iblock` (60 octets max = 4 extents), risquant de lire jusqu'à 786 Ko hors limites sur système corrompu. La boucle est désormais bornée par `min(eh_max, 4)` et protégée contre l'overflow de blocs logiques.

---

## 2. Réfutations Techniques et Vérifications de Sûreté

- **Slots LBA ExoFS (`slot_recovery.rs`) :** L'affirmation que les slots sont lus à des adresses LBA incorrectes (0x100..) est infirmée par le code source : `SLOT_DEFAULT_LBAS` vaut `[8, 16, 24]` secteurs de 512B, soit exactement 4 Ko (`EPOCH_SLOT_A_OFFSET`) et 8 Ko (`EPOCH_SLOT_B_OFFSET`).
- **Éviction d'Inodes POSIX (`inode_emulation.rs`) :** L'inode racine `INO_ROOT` ainsi que les répertoires bénéficient déjà d'une immunité stricte contre l'éviction round-robin (`table[i].ino != INO_ROOT && (flags & DIRECTORY) == 0`).
- **Émulation de Disque Vierge (`boot_recovery.rs`) :** Le fallback tolérant `InvalidMagic` lors du boot est un mécanisme requis pour autoriser le premier formatage sur un média non partitionné.

---

## 3. Matrice de Validation

| Fichier Modifié | Sous-Système | Gravité | Correction Appliquée | Statut |
| :--- | :---: | :---: | :--- | :---: |
| `kernel/src/ipc/endpoint/registry.rs` | IPC | Critique (P0) | Rejet du détournement de nom d'endpoint (`AlreadyConnected`) | Validé |
| `kernel/src/syscall/table/ipc/channel.rs` | IPC/Syscall | Majeur (P1) | Vérification `has_signal_pending` pour interruption `recv` | Validé |
| `kernel/src/syscall/table/ipc/system_ops.rs` | IPC/Syscall | Majeur (P2) | Contrôle des privilèges sur `sys_exo_log` (root / services) | Validé |
| `kernel/src/ipc/ring/spsc.rs` | IPC | Moyen (P2) | Horloge monotone réelle pour `spsc_wait_reply` | Validé |
| `kernel/src/syscall/table/fs/io.rs` | FS/Syscall | Majeur (P2) | Accumulation multi-iovec sur `sys_readv` socket | Validé |
| `kernel/src/syscall/table/fs/system_misc.rs` | FS/Syscall | Majeur (P2) | Démapping (`do_munmap`) lors d'un shrink `mremap` | Validé |
| `kernel/src/syscall/table/memory.rs` | Syscall | Mineur (P3) | `saturating_add` sur le calcul de pages de `mmap` | Validé |
| `kernel/src/fs/exofs/storage/virtio_adapter.rs` | FS | Majeur (P1) | Relâche du verrou `GLOBAL_DISK` avant I/O + rollback MMIO | Validé |
| `kernel/src/fs/exofs/gc/sweeper.rs` | FS | Majeur (P1) | Suppression effective des blobs zéro-refcount dans le GC | Validé |
| `drivers/fs/src/ext4/extent.rs` | FS/Pilotes | Majeur (P1) | Borne stricte `min(eh_max, 4)` et anti-overflow sur extents | Validé |
| `kernel/src/syscall/table/ipc/endpoints.rs` | IPC/Syscall | Mineur (P3) | Déréférencement du dead owner lors d'un remplacement | Validé |
