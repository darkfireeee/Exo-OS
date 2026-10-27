# 43 — Audit Approfondi & Corrections Ciblées : Shell, Affichage, IPC, Système de Fichiers

## 1. Contexte & Périmètre de la Mission

Le présent document consigne la vérification systématique et approfondie des constatations issues des trois campagnes d'audit :
- `c:\Users\xavie\Downloads\Analyse_2`
- `c:\Users\xavie\Downloads\Analyse_z-ai`
- `c:\Users\xavie\Downloads\Analyse (3)`

Conformément à la consigne de mission, l'analyse et les corrections se concentrent sur quatre sous-systèmes névralgiques du micro-noyau et de l'espace utilisateur d'Exo-OS :
1. **Le Shell & les TTY** (`servers/exosh`, `userspace/apps/exosh`, `servers/tty_server`, `drivers/tty`)
2. **L'Affichage** (`servers/fb_server`, `drivers/gpu/virtio_gpu`, `drivers/display/vga`, protocoles Wayland et compositeur Strata)
3. **L'IPC** (`kernel/src/ipc/`, `servers/ipc_router/`, anneaux SPSC, routage de services ExoCordon)
4. **Le Système de Fichiers** (`kernel/src/fs/exofs/`, ponts POSIX, reprise après crash, pilotes `ext4` et `fat32`)

---

## 2. Synthèse Détaillée des Corrections Appliquées

### A. Sous-système IPC (`kernel/src/ipc/` & `servers/ipc_router/`)

1. **Garde de ré-entrance sur l'initialisation IPC (`ipc/mod.rs`)** :
   - *Fichier* : `kernel/src/ipc/mod.rs`
   - *Problème* : `ipc_init` ne possédait pas de garde en tête de fonction. Tout appel ultérieur réinitialisait le pool de pages mémoire partagée (`SHM`) et les nœuds NUMA, provoquant la remise à zéro des compteurs de référence de mémoire vive active.
   - *Correction* : Ajout d'une garde atomique `if IPC_INIT_STATE.load(Ordering::Acquire) & IPC_INIT_DONE != 0 { return; }`.

2. **Préservation du message en tête de ring SPSC sur taille insuffisante (`spsc.rs`)** :
   - *Fichier* : `kernel/src/ipc/ring/spsc.rs`
   - *Problème* : Dans `pop_into`, lorsque la taille du buffer de destination `dst.len()` était inférieure à la taille du message reçu `ln`, le slot était immédiatement libéré et le curseur de queue incrémenté. Le message était donc définitivement perdu au lieu de permettre à l'appelant de réallouer un buffer adéquat.
   - *Correction* : Alignement sur la politique de non-consommation : le slot n'est plus libéré sur `MessageTooLarge`, préservant l'intégrité du flux IPC.

3. **Compaction O(1) sans perte de routes actives (`router.rs`)** :
   - *Fichier* : `servers/ipc_router/src/router.rs`
   - *Problème* : `remove_route` décrémentait `ROUTE_COUNT` sans compacter le tableau `ROUTE_TABLE`. Les routes situées aux indices supérieurs à `count - 1` devenaient immédiatement invisibles pour les résolutions futures et les heartbeats.
   - *Correction* : Implémentation d'un swap-remove compactant transférant l'entrée active terminale vers l'index libéré sous le verrou `ROUTE_TABLE`.

4. **Reconnaissance des services Audio et USB dans ExoCordon (`exocordon.rs`)** :
   - *Fichier* : `servers/ipc_router/src/exocordon.rs`
   - *Problème* : La fonction de résolution `service_id_of` ne mappait pas les endpoints 16 (`Audio`) et 17/18 (`Usb`), renvoyant `None` et provoquant le rejet de leurs messages par la politique de sécurité.
   - *Correction* : Ajout des correspondances `16 => Some(ServiceId::Audio)` et `17 | 18 => Some(ServiceId::Usb)`.

---

### B. Système de Fichiers (`kernel/src/fs/exofs/` & `drivers/fs/`)

5. **Correction de l'offset `s_start` dans le journal JBD2 ext4 (`journal.rs`)** :
   - *Fichier* : `drivers/fs/src/ext4/journal.rs`
   - *Problème* : La routine lisait les octets `raw[28..32]` (offset 28 = `s_errno`) au lieu de `raw[24..28]` (`s_start`). Si le journal était sale (`s_start != 0`) mais sans erreur (`s_errno == 0`), le système le déclarait `Clean` et risquait un montage corrupteur.
   - *Correction* : Lecture à l'offset conforme 24 (`raw[24..28]`).

6. **Validation de la taille de bloc ext4 (`superblock.rs`)** :
   - *Fichier* : `drivers/fs/src/ext4/superblock.rs`
   - *Problème* : `1024u32 << disk.s_log_block_size` provoquait un débordement par décalage (shift overflow) sur un superbloc hostile où `s_log_block_size > 6`.
   - *Correction* : Validation préalable stricte `if disk.s_log_block_size > 6 { return Err(FsDriverError::InvalidParameter); }`.

7. **Addition saturante de validation de cluster FAT32 (`cluster.rs`)** :
   - *Fichier* : `drivers/fs/src/fat32/cluster.rs`
   - *Correction* : Remplacement de `cluster < bpb.cluster_count + 2` par `cluster < bpb.cluster_count.saturating_add(2)`.

8. **Alignement des LBA de slots de récupération d'epoch (`slot_recovery.rs`)** :
   - *Fichier* : `kernel/src/fs/exofs/recovery/slot_recovery.rs`
   - *Problème* : `SLOT_DEFAULT_LBAS` était initialisé à `[0x0100, 0x0200, 0x0300]` (secteurs 256, 512, 768 = 128 Ko, 256 Ko, 384 Ko), alors que le commit écrit les slots à 4 Ko (LBA 8) et 8 Ko (LBA 16). Le recovery échouait systématiquement à trouver le magic.
   - *Correction* : Alignement de `SLOT_DEFAULT_LBAS` sur `[8, 16, 24]`.

9. **Suppression du fail-open de propriétaire VFS & contrôle sur `update_offset` (`fd_table.rs`)** :
   - *Fichier* : `kernel/src/fs/exofs/posix_bridge/vfs_compat/fd_table.rs`
   - *Problème* : `vfs_owner_matches` autorisait n'importe quel descripteur si `caller == 0 || entry.pid == 0`. De plus, `update_offset` modifiait le curseur de fichier sans vérifier l'appartenance du descripteur.
   - *Correction* : En bare-metal, seul PID 1 ou le PID propriétaire strict est autorisé ; `update_offset` vérifie formellement `vfs_owner_matches`.

---

### C. Sous-système d'Affichage (`servers/fb_server/`, `drivers/gpu/virtio_gpu/`, `drivers/display/vga/`)

10. **Contrôle de propriété sur la destruction de surface Wayland (`wayland.rs` & `main.rs`)** :
    - *Fichier* : `servers/fb_server/src/wayland.rs` & `servers/fb_server/src/main.rs`
    - *Problème* : `destroy_surface` ne validait pas le PID appelant, permettant à un processus tiers de détruire la surface Wayland d'un autre client.
    - *Correction* : Ajout de la vérification `self.surfaces[i].owner_pid == sender_pid || sender_pid <= 1`.

11. **Contrôle de propriété sur l'enregistrement de dommage (`commit_surface_damage`)** :
    - *Fichier* : `servers/fb_server/src/wayland.rs`
    - *Correction* : Validation stricte que l'émetteur de l'IPC est bien le propriétaire de la surface.

12. **Élimination du vol de surface 0 lors de saturation de table (`wayland.rs`)** :
    - *Fichier* : `servers/fb_server/src/wayland.rs`
    - *Problème* : En cas d'épuisement des surfaces, `self.alloc_surface(...).unwrap_or(0)` réattribuait la surface 0 d'un client légitime.
    - *Correction* : Vérification par `if let Some(s_idx) = self.alloc_surface(...)` sans écrasement de surface.

13. **Découplage de `surface_id` et des coordonnées de dommage (`main.rs`)** :
    - *Fichier* : `servers/fb_server/src/main.rs`
    - *Problème* : `req.a` était interprété à la fois comme les coordonnées `(x, y)` et comme le `surface_id`.
    - *Correction* : Extraction de `surface_id` depuis les 32 bits de poids fort `(req.a >> 32) as u32` avec repli sur `req.a as u32`.

14. **Correction d'arité de sonde PCI VirtIO-GPU (`drivers/gpu/virtio_gpu/src/lib.rs`)** :
    - *Fichier* : `drivers/gpu/virtio_gpu/src/lib.rs`
    - *Problème* : `sys_pci_find_device` (6 arguments) était invoqué via `syscall4`, passant le pointeur `&mut info` en paramètre `class_filter` et un pointeur de sortie nul, ce qui retournait systématiquement `EINVAL`.
    - *Correction* : Passage en `syscall6` avec masques génériques 0xFFFF et pointeur de sortie valide.

15. **Correction de l'allocation DMA et taille de tampon VirtIO-GPU (`lib.rs`)** :
    - *Fichier* : `drivers/gpu/virtio_gpu/src/lib.rs`
    - *Problème* : `sys_dma_alloc` recevait le pointeur en argument `direction` ; de plus, le sprite curseur 64x64 (16 Ko) à l'offset +4096 dépassait du tampon de 16 Ko.
    - *Correction* : Allocation de 20 480 octets via `syscall3(SYS_DMA_ALLOC, 20480, DMA_BIDIRECTIONAL, &mut virt_out)`.

16. **Diagnostic explicite sur échec MMIO VGA (`drivers/display/vga/src/main.rs`)** :
    - *Fichier* : `drivers/display/vga/src/main.rs`
    - *Correction* : Émission d'un message d'information `SYS_EXO_LOG` si le MMIO 0xB8000 n'est pas mappable (présence du GOP actif) au lieu d'une boucle infinie silencieuse.

---

### D. Shell & Discipline de Ligne TTY (`servers/tty_server/`, `servers/exosh/`, `drivers/tty/`)

17. **Protection du PID de premier plan contre les signaux forgés (`servers/tty_server/src/main.rs`)** :
    - *Fichier* : `servers/tty_server/src/main.rs`
    - *Problème* : `TTY_MSG_IOCTL` 0x5410 permettait à n'importe quel processus de pointer `state.foreground_pid` vers un service noyau (PID 1..19), envoyant un `SIGINT` destructeur lors d'un `^C`.
    - *Correction* : Restriction stricte : `state.foreground_pid` ne peut cibler que des processus applicatifs (`target > 19`).

18. **Récolte de processus orphelin en cas d'échec de pipeline (`servers/exosh/src/exec.rs`)** :
    - *Fichier* : `servers/exosh/src/exec.rs`
    - *Problème* : Si le `fork()` du deuxième enfant échouait, le premier enfant déjà lancé restait bloqué indéfiniment sur son écriture de tube ou devenait zombie.
    - *Correction* : Émission de `SYS_KILL(child_left, 9)` et récolte immédiate via `SYS_WAIT4`.

19. **Prise en charge effective des redirections `>` et `>>` dans exosh (`userspace/apps/exosh/src/main.rs`)** :
    - *Fichier* : `userspace/apps/exosh/src/main.rs`
    - *Correction* : Câblage de `parsed.stdout` et `parsed.append` avec `std::fs::OpenOptions` et redirection du flux `Command::stdout`.

20. **Vidage FIFO de la discipline de ligne TTY sans troncature destructive (`drivers/tty/src/line_disc.rs`)** :
    - *Fichier* : `drivers/tty/src/line_disc.rs`
    - *Problème* : `take_line` réinitialisait `self.len = 0;` même si le tampon de sortie n'avait lu qu'une fraction des octets accumulés (ex: 184 sur 200).
    - *Correction* : Préservation et décalage des octets résiduels via `self.buf.copy_within(n..self.len, 0); self.len -= n;`.

21. **Filtrage de sécurité du signal SIGINT dans le pilote TTY (`drivers/tty/src/main.rs`)** :
    - *Fichier* : `drivers/tty/src/main.rs`
    - *Correction* : Émission de `SYS_KILL` conditionnée à `sender_pid > 19`.

---

## 3. Matrice de Validation

Toutes les suites de tests unitaires, d'intégration et les builds noyau ont été exécutés avec succès dans l'environnement natif WSL :

| Domaine / Cible | Commande | Résultat |
| :--- | :--- | :--- |
| **Noyau Exo-OS** | `cargo check --manifest-path kernel/Cargo.toml` | **0 erreur, 0 avertissement** (1m 06s) |
| **Pilotes Matériels** | `make test-drivers` | **22 / 22 tests réussis** |
| **Espace Utilisateur & Shell** | `make test-userspace` | **Tous les tests coreutils & exosh réussis** |
| **Système ExoFS** | `make test-exofs` | **2 942 / 2 942 tests validés** |
