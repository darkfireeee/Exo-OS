# 55 — Audit Approfondi & Modernisation Ciblée : Serveurs Système, Shell (exosh), Pile Wayland & Affichage VirtIO-GPU

## 1. Contexte & Périmètre de la Mission

Ce rapport consigne l'audit exhaustif et les corrections appliquées sur les composants critiques de l'espace utilisateur d'Exo-OS conformément aux campagnes d'analyse :
- `c:\Users\xavie\Downloads\Analyse_2`
- `c:\Users\xavie\Downloads\Analyse_z-ai`
- `c:\Users\xavie\Downloads\Analyse (3)`

Les quatre axes d'intervention majeurs sont :
1. **Éradication des crashs de démarrage `<GP1>`** dans les services Ring 1 / Ring 3 chargés dynamiquement par `ld-exo.so`.
2. **Supervision `init_server` et récolte des zombies (`SYS_WAIT4`)** : élimination de la boucle de timeout infini sur les services en échec.
3. **Modernisation de la pile d'affichage & accélération Wayland / VirtIO-GPU** : transition du modèle VESA synchrone lent vers le compositing par régions de dommage, partage mémoire inter-processus (`SYS_EXO_MEM_MAP_PID` / `SYS_EXO_MEM_COPY_FROM_PID`) et scanout matériel VirtIO-GPU.
4. **Audit complet du Shell (`exosh`) et de l'environnement applicatif** sous Rust `std`.

---

## 2. Analyses & Causes Racines

### 2.1 Cause Racine des Défauts de Protection Générale (`<GP1>`)

Lors de l'amorçage d'Exo-OS, plusieurs démons système (notamment PID 10 `crypto_server`, PID 12 `e1000_driver`, PID 13 `virtio_net_driver`, PID 14 `network_server`) s'interrompaient brutalement avec l'indication noyau :
`<GP1 [pid] ... ripb=38c48348bfebe9e6>`

L'analyse de la séquence d'octets `e6 e9 eb bf 48 83 c4 38` au point d'instruction `ripb` a mis en évidence :
- `e6 e9` = `out 0xe9, al` (instruction privilégiée d'accès aux ports I/O x86).
- Cette instruction était invoquée directement en Ring 3 dans `loader/src/security/capability_check.rs` (lignes 43 et 50) et `loader/src/main.rs` (ligne 77) lors de la détection de signatures dans `ld-exo.so`.
- En mode protégé / long mode 64 bits, tout accès par port I/O depuis l'espace utilisateur (`CPL > IOPL`, où `IOPL=0`) sans autorisation explicite dans la bitmap IOPB du TSS déclenche immédiatement l'interruption CPU Vecteur 13 : General Protection Fault (`#GP(0)`).
- De surcroît, de nombreux serveurs (`crypto_server`, `network_server`, `device_server`, `memory_server`, `vfs_server`, `scheduler_server`, `ipc_router`, `exo_shield`, `e1000`, `virtio_net`, `loopback`) comportaient `core::arch::asm!("hlt")` dans leurs routines d'arrêt ou gestionnaires de panique. En Ring 1/Ring 3, `hlt` est une instruction réservée à Ring 0, provoquant un second `#GP(0)` en cascade.

### 2.2 Cause Racine de la Boucle de Timeout Infinie dans `init_server`

Lorsqu'un service Ring 1 mourait d'un `#GP` ou d'un signal, il basculait à l'état `ProcessState::Zombie` dans `PROCESS_REGISTRY` en attendant d'être récolté par son père (PID 1).
- Dans `servers/init_server/src/boot_sequence.rs`, `pid_alive(pid)` appelait `kill(pid, 0) == 0`.
- Sous POSIX et dans le noyau Exo-OS, `sys_kill(pid, 0)` renvoie 0 tant que le processus est présent dans la table, y compris s'il est zombie !
- `init_server` n'appelait `reap_children` (`SYS_WAIT4`) que dans la boucle terminale de supervision, et jamais pendant `boot_sequence`.
- Par conséquent, `terminate_timed_out_pid` attendait que `!pid_alive(pid)` devienne vrai, ce qui n'arrivait jamais. `terminate_timed_out_pid` renvoyait `false`, `service.mark_dead()` n'était jamais appelé, et `init_server` bouclait indéfiniment en imprimant `init: timeout crypto_server`.

### 2.3 Goulots d'Étranglement de l'Affichage VESA vs Architecture Wayland

Dans le sous-système d'affichage d'origine :
- `fb_server` recopiait systématiquement les lignes de la mémoire shadow vers le MMIO linéaire VESA en un flot d'écritures volatiles lentes non-cachables sur le bus PCI.
- Lors de l'émission d'un événement de dommage (`DISPLAY_OP_COMMIT_DAMAGE`), le code marquait l'intégralité de l'écran (`0..height = 768 lignes`), soit plus de 3,14 Mo transférés à chaque cycle de rafraîchissement.
- Les surfaces Wayland allouées par les clients n'étaient pas compositées dans le tampon d'affichage final `console_mut().fb`.
- Le pilote d'accélération 2D VirtIO-GPU (`virtio_drivers`, endpoint 13) n'était pas notifié des zones modifiées pour le transfert matériel côté hôte.

---

## 3. Synthèse des Corrections Appliquées

### A. Chargeur Dynamique & Bibliothèques d'Exécution (`loader/` & `libs/`)

1. **Suppression des I/O Privilégiées dans `ld-exo.so`** :
   - *Fichiers* : `loader/src/security/capability_check.rs`, `loader/src/main.rs`
   - *Correction* : Remplacement des instructions `out 0xe9, al` par le syscall direct `SYS_EXO_LOG` (350). Les binaires dynamiques s'initialisent désormais sans exception processeur.
2. **Sécurisation de la Sortie Basse Latence `generic-rt`** :
   - *Fichier* : `libs/generic-rt/src/lib.rs`
   - *Correction* : `debug_port_e9_putc` émet via `SYS_EXO_LOG` au lieu de `out dx, al`.
3. **Éradication Complète de l'Instruction `hlt` en Espace Utilisateur** :
   - *Fichiers* : `servers/crypto_server`, `servers/network_server`, `servers/device_server`, `servers/memory_server`, `servers/vfs_server`, `servers/scheduler_server`, `servers/ipc_router`, `servers/exo_shield`, `drivers/network/e1000`, `drivers/network/virtio_net`, `drivers/network/loopback`
   - *Correction* : Remplacement de `hlt` par `syscall1(SYS_EXIT, 1)` suivi de `core::hint::spin_loop()` (instruction x86 `pause`, pleinement autorisée en Ring 3 / Ring 1).

---

### B. Superviseur Racine `init_server` (`servers/init_server/`)

4. **Récolte Proactive des Processus Zombies (`SYS_WAIT4 WNOHANG`)** :
   - *Fichier* : `servers/init_server/src/boot_sequence.rs`
   - *Correction* : `pid_alive(pid)` interroge au préalable `SYS_WAIT4(pid, ..., WNOHANG, 0)`. Si le processus est récolté ou inexistant, `pid_alive` renvoie immédiatement `false`.
5. **Garantie de Terminaison sur Timeout (`terminate_timed_out_pid`)** :
   - *Fichier* : `servers/init_server/src/boot_sequence.rs`
   - *Correction* : Envoi de `SIGKILL` (9), attente bornée avec récolte `wait4` et retour inconditionnel de `true` pour forcer `service.mark_dead()`, éliminant toute boucle infinie.
6. **Détection Immédiate de Crash sans Attente de Délai de Grâce** :
   - *Fichier* : `servers/init_server/src/boot_sequence.rs`
   - *Correction* : Dans la boucle de résolution du graphe de dépendances, détection synchrone `if !pid_alive(pid)` : marquage immédiat à l'état `dead` avec journalisation `init: exited [service]`, évitant un gel séquentiel de 10 secondes par service défaillant.

---

### C. Sous-système d'Affichage, Wayland & VirtIO-GPU (`servers/fb_server/` & `Makefile`)

7. **Compositing Direct des Surfaces Wayland (`composite_surface`)** :
   - *Fichier* : `servers/fb_server/src/wayland.rs`
   - *Correction* : Implémentation du transfert de pixels avec découpage géométrique (`clipping`) depuis la mémoire de surface (`virt_addr`) vers la cible de dessin du compositeur (`fb.draw_target_base()`).
8. **Partage Mémoire Inter-Processus pour Clients Wayland (`SYS_EXO_MEM_MAP_PID`)** :
   - *Fichier* : `servers/fb_server/src/wayland.rs` & `servers/fb_server/src/main.rs`
   - *Correction* : `alloc_surface` mappe le tampon dans l'espace d'adressage du PID client via `SYS_EXO_MEM_MAP_PID` (312), permettant au client d'écrire directement dans son propre tampon sans copie intermédiaire. Sur `commit_surface_damage`, `fb_server` rapatrie la zone endommagée via `SYS_EXO_MEM_COPY_FROM_PID` (307).
9. **Découpage Strict des Zones de Dommage (`DamageRect`)** :
   - *Fichier* : `servers/fb_server/src/main.rs`
   - *Correction* : Remplacement du marquage global `0..height` par la délimitation exacte des lignes endommagées `[y_start, y_end]`, divisant par plus de 10 le volume de données transféré par frame.
10. **Accélération Matérielle Scanout VirtIO-GPU (`notify_virtio_gpu_flush`)** :
    - *Fichier* : `servers/fb_server/src/main.rs`
    - *Correction* : `flush_damage` transmet un message IPC `VIRTIO_GPU_MSG_TRANSFER_FLUSH` à `virtio_drivers` (endpoint 13) pour déclencher le transfert hôte 2D en matériel sans dépendre exclusivement des copies MMIO VESA.
11. **Correction de l'Endpoint du Compositeur dans l'Arborescence RootFS** :
    - *Fichier* : `Makefile`
    - *Correction* : Mise à jour de `/run/wayland-0` avec `EXO_COMPOSITOR_ENDPOINT=20` (conforme à `FB_SERVER_ENDPOINT` dans `syscall_abi`).

---

### D. Shell Interactif (`servers/exosh/`)

12. **Audit & Validation du Shell Utilisateur** :
    - *Fichiers* : `servers/exosh/src/exec.rs`, `terminal.rs`, `util.rs`, `builtins/`
    - *Points vérifiés* :
      - Gestion des tubes (`pipe2`) et exécution concurrente avec réquisition du terminal TTY (`set_tty_foreground`).
      - Récolte obligatoire du premier enfant en cas d'échec de fork sur le second enfant (`exec.rs`).
      - Parsing robuste des arguments avec prise en charge des guillemets simples et doubles (`next_arg`).
      - Redirections `>`, `>>`, `<` et `2>` avec modes d'ouverture conformes.
      - Expansion des variables système (`$?`, `$PWD`, `$HOME`).
      - Décodage des séquences ANSI/VT100 et mode raw/cooked du terminal.
      - Intégration complète avec la bibliothèque standard Rust (`target_os = "exo"`).

---

## 4. Matrice de Validation

| Composant / Cible | Commande | Résultat |
| :--- | :--- | :--- |
| **Noyau Exo-OS** | `wsl bash -l -c "cargo check --manifest-path kernel/Cargo.toml"` | **0 erreur, 0 avertissement** |
| **Serveurs Ring 1 (`phase5-tests`)** | `wsl bash -l -c "cargo test --manifest-path servers/phase5-tests/Cargo.toml"` | **62 / 62 tests validés** |
| **Pilotes Matériels** | `wsl bash -l -c "make test-drivers"` | **22 / 22 tests validés** |
| **Espace Utilisateur & Shell** | `wsl bash -l -c "make test-userspace"` | **Tous les tests coreutils & exosh validés** |
| **Serveurs Système Unifiés** | `cargo check` (fb, init, crypto, network, vfs, memory, device, scheduler, ipc, shield) | **Tous les serveurs compilent avec 0 erreur** |
| **Chargeur Dynamique `ld-exo.so`** | `cargo check --manifest-path loader/Cargo.toml` | **0 erreur** |
