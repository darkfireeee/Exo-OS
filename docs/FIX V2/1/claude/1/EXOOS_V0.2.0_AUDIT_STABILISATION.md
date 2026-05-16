# ExoOS v0.2.0 — Audit de stabilisation : incohérences et corrections requises

**Auteur :** claude-beta  
**Version source analysée :** v0.1.0 → cible v0.2.0  
**Périmètre :** kernel/, servers/, drivers/ (analyse complète des fichiers non vides)  
**Date :** 2026-05-15  

---

## Résumé exécutif

La v0.1.0 constitue une base solide. Le flux de boot est cohérent de `_start` jusqu'à PID 1, les couches mémoire/scheduler/IPC/sécurité sont architecturalement correctes, et les bugs historiques majeurs (SYSRETQ canonicité, dualité compteur préemption, init_ap() manquant, boot_sequence.rs vide) ont été corrigés. Cependant, l'analyse révèle **trois régressions P0 bloquantes pour v0.2.0** — dont deux rendent le réseau et le VFS inutilisables en production — ainsi que **six anomalies P1** et **six points P2** de polissage.

La v0.2.0 étant la version de stabilisation complète avant l'intégration Wayland et l'installateur visuel, ces points doivent être résolus **avant** le gel du code.

---

## P0 — Bloquants absolus

### P0-NET-01 · SmoltcpIface : SocketSet éphémère, connexions TCP impossibles

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

`poll_one()` et `poll_egress()` instancient un `SocketSet` **local** vide à chaque appel :

```rust
// Dans poll_one() :
let mut sockets = [const { SocketStorage::EMPTY }; SOCKET_STORAGE_LEN];
let mut socket_set = SocketSet::new(&mut sockets[..]);
iface.poll_ingress_single(now, &mut smol_device, &mut socket_set);

// Dans poll_egress() :
let mut sockets = [const { SocketStorage::EMPTY }; SOCKET_STORAGE_LEN];
let mut socket_set = SocketSet::new(&mut sockets[..]);
iface.poll_egress(now, &mut smol_device, &mut socket_set);
```

smoltcp gère l'intégralité de l'état TCP (fenêtre de congestion, numéros de séquence, retransmissions, TIME_WAIT) dans le `SocketSet`. Or ce SocketSet est détruit à la fin de chaque appel à `poll_one()` / `poll_egress()`. Résultat : **aucune connexion TCP ne peut être établie ni maintenue**. Le `TcpStateStore` et la `SocketTable` de `NetworkService` opèrent dans un espace sémantique entièrement déconnecté de smoltcp. Les opérations `NET_OP_CONNECT`, `NET_OP_ACCEPT`, `NET_OP_SENDTO` et `NET_OP_RECVFROM` mettent à jour uniquement l'état local ExoOS, tandis que smoltcp ne connaît aucun socket — les paquets TCP ne sont jamais émis ni reçus au niveau protocole.

**Correction requise :** Le `SocketSet` doit être un champ de `SmoltcpIface` persistant entre les appels, et les sockets TCP/UDP ouverts via `SocketTable` doivent être enregistrés dans ce SocketSet lors de leur création et supprimés lors de leur fermeture.

---

### P0-VFS-01 · vfs_server : pointeurs bruts cross-process dans handle_read / handle_write

**Fichier :** `servers/vfs_server/src/main.rs`

Les handlers `handle_read()`, `handle_write()` et `handle_getdents()` extraient un pointeur `buf` (u64) directement du payload IPC et le transmettent aux syscalls noyau :

```rust
fn handle_read(payload: &[u8]) -> VfsReply {
    let fd  = ops::read_u64(payload, 0)?;
    let buf = ops::read_u64(payload, 8)?;   // adresse appartenant au processus APPELANT
    let len = ops::read_u64(payload, 16)?;
    syscall::syscall3(SYS_READ, fd, buf, len) // exécuté dans l'espace de vfs_server (PID 3)
}
```

Ce pointeur appartient à l'espace d'adressage du processus appelant, mais `SYS_READ` est exécuté dans le contexte de `vfs_server` (PID 3). Le kernel interprétera `buf` comme une adresse dans les tables de pages de PID 3 :

- Si l'adresse n'est pas mappée dans PID 3 → SIGSEGV sur vfs_server → crash du serveur VFS → tous les services dépendants perdent leur FS.
- Si l'adresse est mappée dans PID 3 (par coïncidence ou exploit) → les données lues sont écrites dans la mémoire de vfs_server, pas du processus appelant → corruption mémoire silencieuse.

Cette conception est fondamentalement incompatible avec l'architecture microkernel. Dans un microkernel, les transferts de données entre espaces d'adressage doivent passer par des mécanismes de partage mémoire explicites (SHM, copy_to_user côté kernel, ou zero-copy via le bus IPC).

**Correction requise :** `handle_read` et `handle_write` doivent utiliser un buffer intermédiaire alloué dans l'espace de vfs_server, puis transférer les données via un mécanisme IPC dédié (SHM pré-négocié ou `SYS_IPC_COPY_TO` si disponible). À minima, le kernel doit exposer un syscall `SYS_PROCESS_VM_READV` / `SYS_PROCESS_VM_WRITEV` permettant à vfs_server de copier dans l'espace d'un autre processus en vérifiant ses capabilities.

---

### P0-KPTI-01 · Handlers d'interruption non mappés dans la PML4 user

**Fichier :** `kernel/src/memory/virtual/page_table/kpti_split.rs`

`build_user_shadow_pml4()` ne mappe dans la PML4 user que 6 stubs d'entrée spécifiques :

```rust
map_transition_page(source_pml4_phys, user_pml4, syscall_entry_asm       as *const () as u64, …)?;
map_transition_page(source_pml4_phys, user_pml4, syscall_cstar_noop       as *const () as u64, …)?;
map_transition_page(source_pml4_phys, user_pml4, exc_page_fault_handler   as *const () as u64, …)?;
map_transition_page(source_pml4_phys, user_pml4, exc_double_fault_handler as *const () as u64, …)?;
map_transition_page(source_pml4_phys, user_pml4, exc_nmi_handler          as *const () as u64, …)?;
map_transition_page(source_pml4_phys, user_pml4, irq_timer_handler        as *const () as u64, …)?;
```

Les vecteurs IRQ 1–255 non listés (clavier PS/2 sur IRQ 1, AHCI sur IRQ 11, E1000 sur IRQ 11, spurious IRQ, etc.) **ne sont pas mappés**. Lorsqu'une telle interruption hardware survient pendant qu'un thread userspace s'exécute avec CR3 pointant sur la PML4 user, le processeur tente de lire le stub ASM de l'IDT via la PML4 user. La page n'est pas présente → `#PF` (exception 14) → `exc_page_fault_handler` est bien mappé mais sa pile IST ne l'est peut-être pas — ou un double fault survient → `exc_double_fault_handler` → triple fault → reset machine sans diagnostic.

Sur QEMU avec `-machine q35` et un réseau actif, cette condition se produit à chaque paquet réseau reçu pendant l'exécution d'un thread utilisateur.

**Correction requise :** Tous les stubs d'entrée IDT (ou au minimum les pages couvrant l'ensemble du vecteur d'interruption de l'IDT) doivent être mappés dans `build_user_shadow_pml4()`. L'approche la plus robuste est de mapper la page entière de l'IDT (déjà fait via `idt_base_addr_for_kpti()`) ainsi que toutes les pages contenant les stubs IRQ (généralement regroupées dans une section `.text.irq`).

---

## P1 — Importantes

### P1-NET-02 · Horodatage smoltcp fictif (compteur d'itérations)

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

```rust
pub fn poll_one(&mut self, …) {
    self.ingress_ticks = self.ingress_ticks.saturating_add(1);
    let now = Instant::from_millis(self.ingress_ticks as i64);
    …
}
```

`ingress_ticks` n'est pas un horodatage temps-réel, mais un compteur d'appels. smoltcp base l'intégralité de ses timers (retransmission TCP RTO, TIME_WAIT 2×MSL, ARP TTL, keepalive) sur cette valeur. En charge légère (peu d'IPC), chaque "milliseconde" smoltcp peut représenter des secondes réelles, retardant les retransmissions. En charge lourde, chaque "milliseconde" peut correspondre à quelques microsecondes réelles, déclenchant des retransmissions agressives et saturant le réseau.

**Correction requise :** Appeler `crate::arch::x86_64::time::ktime::ktime_get_ms()` (ou l'équivalent Ring1 via `SYS_CLOCK_GETTIME`) pour obtenir le temps réel à passer à smoltcp.

---

### P1-NET-03 · Adresse IP statique codée en dur, absence de DHCP

**Fichier :** `servers/network_server/src/main.rs`

```rust
self.iface = SmoltcpIface::init(self.driver.mac(), 0x0a00_020f, 24);
// → 10.0.2.15/24 hardcodé
```

Cette adresse est celle assignée par défaut par QEMU User Networking. Sur hardware réel ou tout autre hyperviseur (KVM bridgé, VirtualBox, VMware), cette adresse sera invalide et le réseau sera inopérant. La v0.2.0 vise la stabilisation complète ; un système sans réseau configurable n'est pas stable.

**Correction requise :** Implémenter un mécanisme de configuration minimale : soit lire l'adresse depuis ExoFS (fichier de configuration `/etc/network.conf`), soit implémenter un client DHCP rudimentaire via smoltcp (`smoltcp::socket::dhcpv4::Socket`).

---

### P1-BOOT-01 · boot_services() sans timeout global ni garde-fou de boucle infinie

**Fichier :** `servers/init_server/src/boot_sequence.rs`

```rust
pub unsafe fn boot_services(services: &[Service]) -> usize {
    let mut progress = true;
    while progress {
        progress = false;
        // Pour chaque service non démarré dont les dépendances sont satisfaites :
        //   spawn → wait_for_ipc_ready (avec timeout par service)
        //   si spawn retourne 0 : on continue sans signaler progress=false
    }
    …
}
```

Si un service critique (par exemple `ipc_router`) échoue systématiquement au `spawn` (fork retourne une erreur noyau persistante), tous les services qui en dépendent ne peuvent jamais démarrer. La boucle externe s'arrête dès que `progress` reste faux pendant un tour complet — ce qui est correct — mais si un unique service réussit à démarrer dans chaque itération, la boucle peut tourner `O(n²)` fois avec des `wait_for_ipc_ready` qui expirent chacun. En l'absence de timeout global, le boot se bloque sans diagnostic sur le port 0xE9 ni sur l'écran framebuffer.

**Correction requise :** Ajouter un timeout global de phase (ex. 30 secondes via `SYS_CLOCK_GETTIME`). Après expiration, loguer l'état de chaque service (démarré/échec/en attente) et soit passer en mode dégradé, soit déclencher un kernel panic explicite avec le masque de services actifs.

---

### P1-PHOENIX-01 · PHOENIX_STATE reste à BootStage0 après le boot

**Fichier :** `kernel/src/exophoenix/mod.rs`, `kernel/src/lib.rs`

```rust
pub static PHOENIX_STATE: AtomicU8 = AtomicU8::new(PhoenixState::BootStage0 as u8);
```

L'état `BootStage0` est l'état initial. D'après la documentation interne (EXONET_V4_AUDIT.md), la machine d'états Phoenix doit passer en `Normal` une fois le boot terminé. Or, nulle part dans `kernel_init()` ni dans `userspace_boot::boot_userspace()` l'état n'est mis à jour vers `Normal`. Le module `network_server/isolation.rs` qui implémente `Normal → Draining → Serialized → Normal` repose sur la lecture de `PHOENIX_STATE` pour autoriser ou bloquer le trafic ; si l'état reste `BootStage0`, la logique d'isolation réseau est indéfinie (ni Normal, ni Draining).

**Correction requise :** Dans `boot_userspace()`, après le retour de `create_init_process_from_elf()` avec succès, appeler `crate::exophoenix::set_state(PhoenixState::Normal)`. Ajouter une assertion dans `network_server` au démarrage pour vérifier que le state est bien `Normal` avant d'accepter du trafic.

---

### P1-SEC-01 · SYSCALL_ERROR_COUNT jamais incrémenté

**Fichier :** `kernel/src/arch/x86_64/syscall.rs`

```rust
static SYSCALL_ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn syscall_error_count() -> u64 {
    SYSCALL_ERROR_COUNT.load(Ordering::Relaxed) // retourne toujours 0
}
```

`SYSCALL_ERROR_COUNT` est déclaré et exposé publiquement mais n'est jamais incrémenté. Aucun chemin dans `syscall_rust_handler()` ni dans `dispatch()` ne met à jour ce compteur. Tout outil de monitoring ou de test de régression qui s'appuie sur `syscall_error_count()` pour détecter des erreurs systémiques retournera toujours 0, masquant des régressions potentiellement critiques.

**Correction requise :** Dans `syscall::dispatch::dispatch()`, si le résultat est une valeur négative (erreur errno), incrémenter `SYSCALL_ERROR_COUNT`.

---

### P1-DEP-01 · Dépendances de services : vérification non-transitive

**Fichier :** `servers/init_server/src/service_table.rs`, `servers/init_server/src/dependency.rs`

```rust
const DEPS_CRYPTO: &[&str] = &["vfs_server"];  // ipc_router absent
```

`supervisor::can_start()` vérifie uniquement que les dépendances **directes** d'un service sont actives (PID non nul). `crypto_server` déclare comme seule dépendance `vfs_server`, mais utilise IPC pour s'enregistrer auprès de `ipc_router`. La dépendance transitive (vfs_server dépend de ipc_router) n'est pas explorée par `can_start()`. Si `ipc_router` démarre lentement et que `vfs_server` signale son endpoint avant que `ipc_router` soit totalement stable, `crypto_server` tentera de s'enregistrer sur un routeur partiellement initialisé.

**Correction requise :** Ajouter `"ipc_router"` à `DEPS_CRYPTO`. Appliquer la même vérification à tous les services qui utilisent IPC (pratiquement tous) : toute dépendance implicite sur `ipc_router` doit être déclarée explicitement.

---

## P2 — Polissage et solidité

### P2-VFS-01 · Autorisation mount/umount basée sur sender_pid sans garantie noyau

**Fichier :** `servers/vfs_server/src/main.rs`

```rust
fn handle_mount(sender_pid: u32, payload: &[u8]) -> VfsReply {
    if sender_pid != 1 { return VfsReply { status: syscall::EPERM, … }; }
```

L'autorisation repose sur `sender_pid == 1`. Ce champ est extrait du champ IPC sans qu'il soit documenté que le kernel garantit son authenticité. Si `SYS_IPC_SEND` permet à un processus de choisir librement son `sender_pid` dans l'enveloppe, n'importe quel processus peut monter/démonter des filesystems. Il convient de vérifier dans `kernel/src/ipc/` que le kernel force `sender_pid` à la valeur réelle du processus émetteur.

**Correction requise :** Documenter explicitement dans `kernel/src/ipc/core/types.rs` que `sender_pid` est forcé par le kernel. Si ce n'est pas le cas, l'implémenter en priorité.

---

### P2-NET-01 · Verrouillage spin par opération IPC dans network_server

**Fichier :** `servers/network_server/src/main.rs`

```rust
static NETWORK_SERVICE: Mutex<NetworkService> = Mutex::new(NetworkService::new());

loop {
    let n = recv_raw(&mut raw)?;
    let reply = NETWORK_SERVICE.lock().dispatch(msg); // lock #1
    NETWORK_SERVICE.lock().tick();                    // lock #2 (inutile)
}
```

`tick()` est appelé séparément du `dispatch()`, acquérant le `spin::Mutex` une seconde fois inutilement par itération. Sur une boucle IPC à haute fréquence, ce double-lock représente une pression spin non négligeable. De plus, si `dispatch()` prend du temps (e.g. attente DMA), `tick()` est différé, bloquant potentiellement les retransmissions smoltcp.

**Correction requise :** Fusionner `dispatch()` et `tick()` dans un seul appel sous lock, ou restructurer pour que le tick soit systématiquement effectué en fin de `dispatch()`.

---

### P2-NET-02 · ExoSmoltcpDevice utilise *mut ExoNetDevice (pointeur brut)

**Fichier :** `servers/network_server/src/smoltcp_iface.rs`

```rust
struct ExoSmoltcpDevice<'a> {
    device: *mut ExoNetDevice,  // bypasse le borrow checker
    pool: &'a NetBufPool,
}
```

L'utilisation d'un pointeur brut contourne les garanties du borrow checker Rust. Bien que fonctionnel dans ce contexte single-threaded, ce pattern empêche le compilateur de vérifier les invariants d'accès exclusif et génère un `unsafe` implicite dans `device_mut()`.

**Correction requise :** Remplacer par `device: &'a mut ExoNetDevice` et ajuster la durée de vie. Si smoltcp exige des lifetimes distincts pour `RxToken` et `TxToken`, utiliser `core::cell::RefCell<&mut ExoNetDevice>`.

---

### P2-INIT-01 · Backoff de relance potentiellement nul

**Fichier :** `servers/init_server/src/main.rs`

```rust
let delay = SERVICES[i].restart_delay_ticks.load(Ordering::Relaxed);
if delay == 0 || delay == 1 {
    let _ = start_service(i, &mut service_watchdog);
} else {
    SERVICES[i].restart_delay_ticks.fetch_sub(1, Ordering::Relaxed);
}
```

Si `start_service()` réussit (`set_pid()` est appelé), `restart_delay_ticks` est remis à 1. Mais si le service crashe **avant** le prochain tour de boucle, `delay` vaut 1 → relance immédiate → crash → delay 2 → attente 1 tick → crash. En pratique, un service crashant en boucle très rapide peut épuiser les ressources fork sans jamais déclencher le backoff exponentiel efficacement.

**Correction requise :** Ne remettre `restart_delay_ticks` à 1 qu'après qu'un service a survécu un minimum de temps (ex. 5 secondes depuis le spawn). Introduire un `spawn_time: AtomicU64` dans `Service` comparé au temps courant avant de réinitialiser le backoff.

---

### P2-DIAG-01 · Handlers non mappés dans la liste KPTI non documentés

**Fichier :** `kernel/src/memory/virtual/page_table/kpti_split.rs`

La liste des handlers mappés dans `build_user_shadow_pml4()` n'est pas accompagnée d'un commentaire listant les handlers **intentionnellement exclus** et la justification. Un développeur ajoutant un nouveau handler IRQ n'est pas averti qu'il doit aussi l'ajouter ici.

**Correction requise :** Ajouter un commentaire exhaustif listant tous les vecteurs IDT utilisés par ExoOS et indiquer pour chacun s'il est mappé dans la PML4 user ou non, avec justification. Ajouter un test de compilation (`const_assert!` ou test d'intégration) vérifiant que tous les handlers critiques sont présents.

---

### P2-DIAG-02 · sendmsg / recvmsg / socketpair retournent ENOTSUP sans trace

**Fichier :** `servers/network_server/src/main.rs`

```rust
NET_OP_SENDMSG | NET_OP_RECVMSG => NetReply::error(exo_syscall_abi::ENOTSUP),
NET_OP_SOCKETPAIR               => NetReply::error(exo_syscall_abi::ENOTSUP),
```

Ces opcodes retournent `ENOTSUP` sans aucune trace de debug. Un processus POSIX qui appelle `sendmsg()` recevra `ENOTSUP` sans log visible côté noyau ou réseau, rendant le diagnostic très difficile. La cible de compatibilité POSIX ~95% exige au minimum le retour de `EOPNOTSUPP` (équivalent POSIX) plutôt que `ENOTSUP` (POSIX.1-2001 extension).

**Correction requise :** Retourner `EOPNOTSUPP` (errno 95 Linux) plutôt que `ENOTSUP`. Ajouter un compteur statique d'appels à ces opcodes non implémentés, exposé via le protocole de heartbeat ou les stats du service.

---

## Tableau récapitulatif

| ID | Priorité | Composant | Titre court | Impact runtime |
|----|----------|-----------|-------------|----------------|
| P0-NET-01 | **P0** | network_server | SocketSet éphémère → TCP impossible | Réseau TCP inopérant |
| P0-VFS-01 | **P0** | vfs_server | Pointeurs bruts cross-process | Crash vfs_server / corruption mémoire |
| P0-KPTI-01 | **P0** | kernel/kpti_split | IRQ non mappées en PML4 user | Triple fault → reboot machine |
| P1-NET-02 | P1 | network_server | Horodatage smoltcp fictif | Timers TCP incorrects |
| P1-NET-03 | P1 | network_server | IP statique 10.0.2.15 | Réseau inopérant hors QEMU |
| P1-BOOT-01 | P1 | init_server | boot_services sans timeout global | Boot bloqué sans diagnostic |
| P1-PHOENIX-01 | P1 | kernel/exophoenix | PHOENIX_STATE bloqué à BootStage0 | Isolation réseau indéfinie |
| P1-SEC-01 | P1 | kernel/syscall | SYSCALL_ERROR_COUNT jamais incrémenté | Monitoring aveugle |
| P1-DEP-01 | P1 | init_server | Dépendances de services non-transitives | Race condition au boot |
| P2-VFS-01 | P2 | vfs_server | sender_pid non garanti côté kernel | Élévation de privilège potentielle |
| P2-NET-01 | P2 | network_server | Double lock spin par itération | Latence IPC réseau |
| P2-NET-02 | P2 | network_server | *mut ExoNetDevice (pointeur brut) | Sécurité type Rust affaiblie |
| P2-INIT-01 | P2 | init_server | Backoff relance potentiellement nul | Épuisement ressources fork |
| P2-DIAG-01 | P2 | kernel/kpti_split | Handlers exclus non documentés | Maintenabilité KPTI |
| P2-DIAG-02 | P2 | network_server | sendmsg/socketpair → ENOTSUP sans trace | Diagnostic POSIX difficile |

---

## Points validés (non-régressions confirmées)

Les éléments suivants, identifiés comme défauts dans les audits précédents, sont **confirmés corrigés** dans v0.1.0 :

- **SYSRETQ RSP canonicité (CVE-2012-0217)** : `is_user_return_addr()` intercepte les adresses non canoniques avant SYSRETQ et force `frame.rcx = 0`.
- **Dualité compteur préemption** : `PREEMPT_COUNT` dans `scheduler/core/preempt.rs` est le compteur canonique ; `gs:[0x30]` est un miroir ASM mis à jour par `arch_set_preempt_count_shadow()`. Architecture cohérente.
- **`scheduler::init_ap()` non appelé sur les APs** : `ap_entry()` appelle bien `crate::scheduler::init_ap(cpu_id)` à l'étape 6c.
- **`boot_sequence.rs` vide** : le fichier contient la séquence complète `boot_services()`.
- **KPTI PML4[511] full copy** : `build_user_shadow_pml4()` copie uniquement les entrées 0..256 (user-space), PML4[511] (kernel) n'est pas copié.
- **`init_server` lance 12 services** : `SERVICES` contient bien 12 entrées correspondant à la chaîne canonique complète.
- **FPU lazy init** : `scheduler/fpu/lazy.rs` gère correctement CR0.TS et `#NM`.
- **Pile kernel per-CPU dans GSBase** : `init_percpu_for_bsp()` et `init_percpu_for_ap()` initialisent correctement `gs:[0x00]` (kernel_rsp).

---

*Rapport produit par claude-beta — audit statique sur sources v0.1.0. Les corrections P0 sont bloquantes pour le gel v0.2.0.*
