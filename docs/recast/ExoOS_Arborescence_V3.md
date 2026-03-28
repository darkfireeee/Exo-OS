ExoOS
Arborescence Complète — Servers Ring 1 + Drivers
no_std · capability-based · Ring 0/1/3 · ExoFS content-addressed

Version 3  ·  Mars 2026  ·  ExoOS Project
Corrections V1→V2→V3 intégrées — validé par analyse multi-modèles (6 IA × 2 tours)


Changelog V3 — Corrections appliquées
Cette version intègre toutes les corrections identifiées lors des deux tours de validation. Les ajouts sont marqués ✦ V3.

Réf.	Correction	Impact
A1	Ordre de boot unifié — 1 seule table canonique sections 3+7	Élimine la contradiction bloquante
A2	ObjectId opaque [u8;32] sans blake3 dans exo-types	Corrige violation SRV-02 systémique
A3	sigchld_handler.rs + mécanisme IPC ChildDied documenté	Corrige SRV-01 obligatoire
A4	isolation.rs dans 4 servers + PrepareIsolationAck + trigger	ExoPhoenix peut geler proprement
A5	claim_validator.rs + Claim { driver_cap, nonce kernel }	Empêche driver malveillant
A6	Interface exo_shield ↔ Kernel B : syscall 520-521 TRANCHÉ	Supprime l'ambiguïté architecturale
A7	FixedString<N> dans exo-types — promu bloquant	Corrige violation no_std IPC
A8	sender_pid: u32 dans IpcMessage — promu bloquant	IPC request/reply fonctionnel
B3	verify_cap_token() partagé dans exo-types/cap.rs	Évite divergence entre servers
NEW	ObjectId computation flow : crypto_server.hash.rs seul source	Clarifie SRV-04 en pratique
NEW	PrepareIsolationAck + trigger (syscall 521) documentés	ExoShield sait quand geler
NEW	exo-syscall/phoenix.rs : syscalls 520-529 dédiés ExoPhoenix	Interface Ring1↔KernelB spécifiée
NEW	CI enforcement script dans section 6	Détection automatique violations


0. Principes d'architecture
Chaque server Ring 1 et chaque driver est un binaire Rust indépendant, compilé en no_std, sans accès direct au hardware. La communication avec le kernel se fait uniquement via IPC (syscalls ExoOS). Les capabilities contrôlent chaque opération.

Couche	Ring	Caractéristiques
Kernel A/B (ExoPhoenix)	Ring 0	no_std, accès hardware direct, capabilities émises, Kernel B sur Core 0 dédié
Servers	Ring 1	no_std, IPC uniquement, capabilities spécifiques, ExoPhoenix surveille leurs pages
Drivers	Ring 1	no_std, Virtio pour QEMU, accès device via device_server uniquement
Userspace (exo-libc)	Ring 3	POSIX partiel, passe par les servers pour tout accès système

⚠ SRV-02 : aucun crate sauf crypto_server n'importe chacha20poly1305 ou blake3. La règle s'applique aux libs partagées (exo-types, exo-ipc, exo-syscall) autant qu'aux servers. CI obligatoire.
ℹ SRV-04 : toute opération cryptographique (hash Blake3 inclus) passe par crypto_server via IPC. ObjectId calculé uniquement dans crypto_server/hash.rs.
ℹ SRV-01 : init_server reçoit le message IPC ChildDied du kernel quand un processus enfant meurt — sigchld_handler.rs l'achemine vers supervisor.rs.
ℹ IPC-01 : SpscRing utilise #[repr(C, align(64))] sur head et tail — implémenté manuellement, sans crossbeam (indisponible bare-metal).
ℹ IPC-02 (✦ V3) : tous les types dans protocol.rs sont Sized. Utiliser FixedString<N> (exo-types). Aucun &str, Vec<>, String, Box<>.


1. Structure workspace global
Fichier / Répertoire	Rôle
📁 ExoOS/	Workspace racine Cargo
📄 Cargo.toml	Workspace — déclare kernel/, servers/*, drivers/*, libs/*
📁 kernel/	Kernel Ring 0 (existant)
📁 servers/	Servers Ring 1 — à créer
📁 drivers/	Drivers Ring 1 — à créer
📁 libs/	Crates no_std partagées
📁 exo-boot/	Bootloader UEFI (existant)


2. libs/ — Crates partagées no_std
Ces crates définissent les types et interfaces partagés. no_std obligatoire. Pas de heap dans les types IPC — taille fixe uniquement.
⚠ Aucune de ces crates ne doit importer blake3 ou chacha20poly1305. SRV-02 s'applique aux libs autant qu'aux servers.

libs/exo-types/ ✦ MODIFIÉ V3
Fichier / Répertoire	Rôle
📁 exo-types/	Types communs — ObjectId, CapToken, ExoError, FixedString, IpcMessage
📄 Cargo.toml	name='exo-types' no_std — AUCUNE dépendance blake3/chacha20
📁 src/	
📄 lib.rs	#![no_std] — pub use cap, error, object_id, ipc_msg, fixed_string
📄 ✦ cap.rs	CapToken, CapabilityType, Rights + verify_cap_token() utilitaire partagé ✦
📄 error.rs	ExoError — codes d'erreur unifiés tous rings
📄 ✦ object_id.rs	ObjectId([u8;32]) opaque — AUCUN calcul blake3 ici ✦
📄 ✦ ipc_msg.rs	IpcMessage { sender_pid:u32, msg_type:u32, payload:[u8;56] } — 64 bytes ✦
📄 ✦ fixed_string.rs	FixedString<N>, ServiceName([u8;64]), PathBuf([u8;512]) ✦ NOUVEAU

✦ V3 — object_id.rs : ObjectId = [u8;32] opaque, #[repr(C)], Copy, Clone. Aucun import blake3. Le hash est calculé uniquement par crypto_server/hash.rs et retourné comme ObjectId déjà calculé via IPC.
✦ V3 — ipc_msg.rs : IpcMessage { sender_pid: u32, msg_type: u32, payload: [u8; 56] } — 64 bytes total = 1 cache-line. sender_pid renseigné par le kernel à l'envoi, permet les replies directs sans mécanisme tiers.
✦ V3 — fixed_string.rs : FixedString<const N: usize> { bytes:[u8;N], len:usize } — #[repr(C)], Copy, Clone. ServiceName=FixedString<64>, PathBuf=FixedString<512>. Tous les protocol.rs utilisent ces types au lieu de &str/String.
✦ V3 — cap.rs : fn verify_cap_token(token: &CapToken, expected: CapabilityType) — panic! si type invalide. Appelé en première instruction de main.rs de chaque server.

libs/exo-ipc/
Fichier / Répertoire	Rôle
📁 exo-ipc/	Primitives IPC Ring 1
📄 Cargo.toml	name='exo-ipc' no_std
📁 src/	
📄 lib.rs	pub use send, receive, ring
📄 send.rs	ipc_send(pid: u32, msg: IpcMessage) — syscall wrapper
📄 receive.rs	ipc_receive() → IpcMessage — blocking, retourne sender_pid renseigné
📄 ring.rs	SpscRing<T,N> — head/tail avec #[repr(C, align(64))] manuel (IPC-01)

libs/exo-syscall/ ✦ MODIFIÉ V3
Fichier / Répertoire	Rôle
📁 exo-syscall/	Wrappers syscalls ExoOS
📄 Cargo.toml	name='exo-syscall' no_std
📁 src/	
📄 lib.rs	pub use exofs::*, process::*, memory::*, phoenix::*
📄 exofs.rs	Wrappers syscalls 500-519 ExoFS
📄 process.rs	Wrappers fork/exec/exit/wait
📄 memory.rs	Wrappers mmap/munmap/mprotect
📄 ✦ phoenix.rs	Syscalls 520-529 ExoPhoenix — phoenix_query(520), phoenix_notify(521) ✦ NOUVEAU
✦ V3 — phoenix.rs : syscall 520 = phoenix_query() → poll état Kernel B, retourne PhoenixEvent. Syscall 521 = phoenix_notify(AllReady) → signal à Kernel B que tous les servers sont checkpointés et que le gel peut avoir lieu.


3. servers/ — Servers Ring 1
Ordre de démarrage canonique V3 — source unique de vérité. Toute autre liste contradictoire est invalide.

Étape	Server	Condition	Note
1	ipc_broker (PID 2)	Rien — premier toujours	Kernel assigne PID 2 directement au boot
2	memory_server	ipc_broker disponible	Bloque tout userspace jusqu'à dispo
3	init_server (PID 1)	ipc_broker + memory_server	Reçoit CapToken ipc_broker en argv[1] du kernel
4	vfs_server (PID 3)	init_server + ExoFS kernel monté	Requiert ExoFS Phase 4
5	crypto_server (PID 4)	vfs_server disponible	SEUL avec RustCrypto
6	device_server	ipc_broker + memory_server	DOIT précéder tout driver
7	virtio-block	device_server disponible	Backend ExoFS disque
8	virtio-net	device_server disponible	Réseau QEMU
9	virtio-console	device_server disponible	Console debug
10	network_server	virtio-net disponible	Stack réseau
11	scheduler_server	init_server disponible	Politique scheduling
12	exo_shield	Phase 3 ExoPhoenix stable	APRÈS Phase 3 UNIQUEMENT

ℹ Paradoxe PID (✦ V3) : ipc_broker est PID 2 et démarre en étape 1 car le kernel lui assigne ce PID directement au boot, avant tout processus logique. init_server est PID 1 (superviseur logique) mais lancé en étape 3. Le kernel passe la CapToken initiale d'ipc_broker à init_server en argv[1] pour le bootstrap sécurisé — pas de dépendance IPC pour ce premier handshake.

Server	PID	Rôle critique
ipc_broker	2	Directory service — localisation des servers (SRV-05). Démarre en premier.
init_server	1	Superviseur — démarre et redémarre les services. Handler IPC ChildDied (SRV-01).
vfs_server	3	Namespaces de montage, /proc /sys /dev via pseudo_fs/. Interface ExoFS → VFS POSIX.
memory_server	dyn	Allocations mémoire userspace. Interface buddy kernel. Bloque tout userspace.
crypto_server	4	SEUL service avec RustCrypto (SRV-04). Hash Blake3 → ObjectId. Clés, CSPRNG.
device_server	dyn	Cycle de vie des drivers Ring 1. Registre PCI. Claim validé par CapToken+nonce.
scheduler_server	dyn	Politique de scheduling Ring 1/3. Interface avec kernel scheduler.
network_server	dyn	Stack réseau. Dépend de virtio-net + device_server.
exo_shield	dyn	Interface ExoPhoenix ↔ Ring 1. À CRÉER APRÈS PHASE 3.

Pattern de démarrage de chaque server :
•	1. Recevoir CapToken (argv du kernel pour ipc_broker/init_server, via ipc_broker pour les autres)
•	2. verify_cap_token(token, expected_type) — panic! si invalide (exo-types/cap.rs)
•	3. S'enregistrer auprès d'ipc_broker
•	4. Boucle IPC infinie : receive() → vérifier msg.sender_pid → traiter → reply via ipc_send(msg.sender_pid, response)


4. Arborescences détaillées des servers
servers/ipc_broker/ — Directory service (PID 2)	PRIORITÉ 1
Fichier / Répertoire	Rôle
📁 ipc_broker/	
📄 Cargo.toml	name='exo-ipc-broker' binary no_std
📁 src/	
📄 main.rs	verify_cap_token() → enregistrement → boucle IPC
📄 registry.rs	Table services : ServiceName → (pid:u32, CapToken)
📄 directory.rs	Lookup service par ServiceName — reply via sender_pid
📄 protocol.rs	Register { name:ServiceName, cap:CapToken }, Lookup { name:ServiceName }, Deregister
📄 ✦ persistence.rs	Dump registry → ExoFS (ObjectId) périodiquement — survie au crash ✦
ℹ Bootstrap : le kernel passe la CapToken d'ipc_broker à init_server en argv[1]. init_server peut ainsi lier ipc_broker sans IPC établi — résout le paradoxe PID sans dépendance circulaire.

servers/init_server/ — Superviseur PID 1 ✦ MODIFIÉ V3	PRIORITÉ 1
Fichier / Répertoire	Rôle
📁 init_server/	
📄 Cargo.toml	name='exo-init' binary no_std
📁 src/	
📄 main.rs	verify_cap_token() → bootstrap ipc_broker via argv[1] → démarre services → boucle supervision
📄 supervisor.rs	Restart policy — écoute ChildDied, applique restart/abort/ignore par service
📄 service_table.rs	Déclaration ordonnée des services avec restart policy
📄 ✦ protocol.rs	Start, Stop, Status, Restart, ChildDied { pid:u32, exit_code:i32 }, PrepareIsolation, PrepareIsolationAck ✦
📄 ✦ sigchld_handler.rs	Reçoit ChildDied IPC du kernel → achemine vers supervisor.rs (SRV-01) ✦ NOUVEAU
📄 ✦ isolation.rs	Handler PrepareIsolation → flush service_table → ExoFS → PrepareIsolationAck ✦ NOUVEAU
✦ V3 — sigchld_handler.rs : dans ExoOS (microkernel sans signaux UNIX natifs), le kernel envoie un message IPC ChildDied { pid, exit_code } à init_server (PID 1) quand un process enfant se termine. Ce fichier reçoit ce message et l'achemine vers supervisor.rs pour application de la restart policy.
✦ V3 — isolation.rs : à la réception de PrepareIsolation, flush la service_table vers ExoFS (syscall EXOFS_WRITE), enregistre l'ObjectId du checkpoint, retourne PrepareIsolationAck { server: ServiceName::INIT, checkpoint_id: ObjectId } à exo_shield via ipc_send(sender_pid, ack).

servers/vfs_server/ — Namespaces VFS (PID 3) ✦ MODIFIÉ V3	PRIORITÉ 2
Fichier / Répertoire	Rôle
📁 vfs_server/	
📄 Cargo.toml	name='exo-vfs' binary no_std
📁 src/	
📄 main.rs	verify_cap_token() + boucle IPC
📄 mount.rs	Mount namespace — association PathBuf/ExoFS objectId
📄 path_resolver.rs	Résolution PathBuf → ObjectId (wrappe SYS_EXOFS_PATH_RESOLVE)
📄 pseudo_fs.rs	Pseudo-filesystems : /proc /sys /dev (contenu dynamique)
📄 fd_table.rs	Table des descripteurs de fichiers par processus
📄 ✦ protocol.rs	Open { path:PathBuf }, Close, Read, Write, Stat, Readdir, PrepareIsolation, PrepareIsolationAck ✦
📄 ✦ isolation.rs	Handler PrepareIsolation — flush mount_table + fd_table → ExoFS → ack ✦ NOUVEAU
✦ V3 — protocol.rs : Open prend path: PathBuf (= FixedString<512>, Sized, no_std safe). Aucun &str dans les messages IPC.

servers/memory_server/ — Allocations mémoire userspace ✦ MODIFIÉ V3	PRIORITÉ 1
Fichier / Répertoire	Rôle
📁 memory_server/	
📄 Cargo.toml	name='exo-memory' binary no_std
📁 src/	
📄 main.rs	verify_cap_token(CapabilityType::MemoryServer) + boucle IPC
📄 allocator.rs	Interface buddy kernel — Alloc, Free
📄 mmap.rs	Mappings virtuels — MapShared(ObjectId), MapAnon, Unmap, Protect
📄 region_table.rs	Registre des régions allouées par processus
📄 ✦ protocol.rs	Alloc(size:usize), Free(offset:u64), MapShared(ObjectId), Protect, PrepareIsolation, PrepareIsolationAck ✦
📄 ✦ isolation.rs	Handler PrepareIsolation — flush region_table → ExoFS → ack ✦ NOUVEAU
⚠ MapShared retourne offset virtuel userspace (u64) — JAMAIS d'adresse physique dans les messages IPC. Seuls ObjectId et offsets validés autorisés.

servers/crypto_server/ — Seul service avec RustCrypto (PID 4) ✦ MODIFIÉ V3	PRIORITÉ 2
⚠ SRV-04 : seul crypto_server importe chacha20poly1305 et blake3. CI grep interdit dans les autres servers ET dans les libs partagées.
Fichier / Répertoire	Rôle
📁 crypto_server/	
📄 Cargo.toml	name='exo-crypto' binary no_std + blake3 + chacha20poly1305
📁 src/	
📄 main.rs	verify_cap_token() + boucle IPC — NE JAMAIS exposer les clés brutes
📄 rng.rs	CSPRNG — RDRAND + ChaCha20. Seule source d'entropie.
📄 key_store.rs	Stockage clés en mémoire chiffrée. Clé maître dérivée au boot via entropy.rs.
📄 session.rs	Gestion sessions chiffrées inter-processus
📄 ✦ hash.rs	Blake3 sur demande → ObjectId. SEULE source d'ObjectId du système. ✦
📄 ✦ protocol.rs	GenKey, DeriveKey, Encrypt, Decrypt, Hash { data_obj:ObjectId }→ObjectId, GenRandom, PrepareIsolation, PrepareIsolationAck ✦
📄 ✦ entropy.rs	Dérivation clé maître : RDRAND→entropy_pool→HKDF→master_key ✦ NOUVEAU
📄 ✦ isolation.rs	Handler PrepareIsolation — seal active keys chiffrées → ExoFS → ack ✦ NOUVEAU
✦ V3 — hash.rs : crypto_server est la SEULE entité qui calcule un ObjectId. Tout composant ayant besoin d'un ObjectId envoie Hash { payload_ref: ObjectId } à crypto_server (via IPC) et reçoit ObjectId en retour. Ceci respecte SRV-02 et SRV-04 conjointement.
✦ V3 — entropy.rs : séquence boot — RDRAND(64 bytes) → entropy_pool → HKDF(seed=pool, info=b'exoos-v1') → master_key. Documenté pour auditabilité.

servers/device_server/ — Cycle de vie des drivers ✦ MODIFIÉ V3	PRIORITÉ 2
Fichier / Répertoire	Rôle
📁 device_server/	
📄 Cargo.toml	name='exo-device' binary no_std
📁 src/	
📄 main.rs	verify_cap_token() + boucle IPC
📄 pci_registry.rs	Registre PCI — bus/device/func → driver assigné
📄 lifecycle.rs	Start/Stop/Reset driver Ring 1. FLR PCI sur reset.
📄 probe.rs	Découverte et association device↔driver
📄 irq_router.rs	Routage IRQs hardware vers drivers Ring 1 via IPC IrqNotify
📄 ✦ protocol.rs	Probe, Claim { device_id:PciId, driver_cap:CapToken, nonce:u64 }, Release, Reset, IrqNotify ✦
📄 ✦ claim_validator.rs	Valide 3 conditions avant Claim : CapToken valide + PciId autorisé + non déjà claimed ✦ NOUVEAU
✦ V3 — claim_validator.rs : vérifie (1) driver_cap.capability_type() == CapabilityType::Driver(device_id), (2) device_id correspond au PciId autorisé dans la CapToken, (3) device_id absent de pci_registry. Le nonce est généré par le kernel et inclu dans la CapToken — un driver ne peut pas le forger.

servers/scheduler_server/ — Politique scheduling Ring 1/3	PRIORITÉ 3
Fichier / Répertoire	Rôle
📁 scheduler_server/	
📄 Cargo.toml	name='exo-scheduler' binary no_std
📁 src/	
📄 main.rs	verify_cap_token() + boucle IPC
📄 policy.rs	CFS adapté Ring 1/3 — priorités, nice values
📄 thread_table.rs	Table des threads actifs avec leurs priorités
📄 protocol.rs	SetPriority, Yield, GetStat

servers/network_server/ — Stack réseau	PRIORITÉ 3
Fichier / Répertoire	Rôle
📁 network_server/	
📄 Cargo.toml	name='exo-net' binary no_std + smoltcp default-features=false features=['socket-tcp','socket-udp']
📁 src/	
📄 main.rs	verify_cap_token() + boucle IPC
📄 socket.rs	Table des sockets TCP/UDP par processus
📄 routing.rs	Table de routage, ARP
📄 driver_iface.rs	Interface vers virtio-net via device_server
📄 protocol.rs	Socket, Connect, Bind, Send, Recv, Close
ℹ smoltcp : default-features = false, features = ['socket-tcp','socket-udp'] — pour garantir l'absence d'alloc implicite.

servers/exo_shield/ — Interface ExoPhoenix ↔ Ring 1 ✦ MODIFIÉ V3	À CRÉER APRÈS PHASE 3
ℹ exo_shield n'existe pas encore. À implémenter après que Phase 3 ExoPhoenix est complète et stable.
Fichier / Répertoire	Rôle
📁 exo_shield/	CRÉER — pas de fichier existant
📄 Cargo.toml	name='exo-shield' binary no_std
📁 src/	
📄 ✦ main.rs	Poll Kernel B via syscall phoenix_query(520) — boucle événements ExoPhoenix ✦
📄 event_relay.rs	Relais événements ExoPhoenix (Threat, Restore, Emergency) vers Ring 1
📄 isolation_notify.rs	Broadcast PrepareIsolation aux servers abonnés, collecte PrepareIsolationAck
📄 ✦ protocol.rs	PhoenixEvent, PrepareIsolation, PrepareIsolationAck { server:ServiceName, checkpoint_id:ObjectId } ✦
📄 ✦ subscription.rs	Registry des servers abonnés aux événements ExoPhoenix ✦ NOUVEAU
✦ V3 — Interface exo_shield ↔ Kernel B TRANCHÉE : communication via syscall dédié 520-521 (exo-syscall/phoenix.rs). phoenix_query(520) = poll état Kernel B. phoenix_notify(521, AllReady) = signal Kernel B que tous les servers sont checkpointés.
✦ V3 — PrepareIsolation flow complet : Kernel B détecte anomalie → IRQ dédiée → exo_shield (syscall 520) → broadcast PrepareIsolation à servers abonnés (subscription.rs) → chaque server flush état + ObjectId checkpoint → PrepareIsolationAck → exo_shield accumule acks → phoenix_notify(521, AllReady) → Kernel B gèle.


5. drivers/ — Drivers Ring 1
Les drivers sont des binaires Ring 1 indépendants. Ils utilisent le protocole Virtio pour QEMU. Ils s'enregistrent auprès de device_server (qui DOIT être démarré avant eux). ExoPhoenix surveille leurs pages et les rechargera depuis ExoFS si compromis.
⚠ ORDRE CRITIQUE : device_server (étape 6) AVANT tout driver (étapes 7-9). Un driver ne peut pas exécuter Claim si device_server n'existe pas.

Driver	Priorité	Dépendance obligatoire
virtio-block	1 — CRITIQUE	device_server disponible. ExoFS disque. Requis pour forge.rs.
virtio-net	2 — HAUTE	device_server disponible. Réseau QEMU.
virtio-console	3 — HAUTE	device_server disponible. Console debug.

drivers/virtio-block/ — Stockage QEMU	PRIORITÉ 1
Fichier / Répertoire	Rôle
📁 virtio-block/	
📄 Cargo.toml	name='exo-virtio-block' binary no_std
📁 src/	
📄 main.rs	Claim sécurisé auprès de device_server (driver_cap+nonce) + init Virtio + boucle
📄 virtio.rs	Protocole Virtio — feature negotiation, device reset, status, config space access
📄 queue.rs	VirtQueue split ring — descripteur, available ring, used ring
📄 block.rs	Requêtes VIRTIO_BLK_T_IN/OUT — lecture/écriture secteurs
📄 exofs_backend.rs	Enregistrement comme backend de stockage ExoFS

drivers/virtio-net/ — Réseau QEMU	PRIORITÉ 2
Fichier / Répertoire	Rôle
📁 virtio-net/	
📄 Cargo.toml	name='exo-virtio-net' binary no_std
📁 src/	
📄 main.rs	Claim sécurisé + init Virtio + boucle receive/transmit
📄 virtio.rs	Protocole Virtio net — feature negotiation, MAC, MTU
📄 queue.rs	RX queue + TX queue (deux VirtQueues séparées)
📄 net.rs	Interface réseau vers network_server

drivers/virtio-console/ — Console QEMU	PRIORITÉ 3
Fichier / Répertoire	Rôle
📁 virtio-console/	
📄 Cargo.toml	name='exo-virtio-console' binary no_std
📁 src/	
📄 main.rs	Claim sécurisé + init Virtio console + boucle read/write
📄 virtio.rs	Protocole Virtio console — port 0 (stdin/stdout)
📄 console.rs	Interface vers /dev/console dans vfs_server


6. Cargo.toml workspace
[workspace]
members = [
  "kernel", "exo-boot",
  "servers/ipc_broker", "servers/init_server", "servers/vfs_server",
  "servers/memory_server", "servers/crypto_server", "servers/device_server",
  "servers/scheduler_server", "servers/network_server", "servers/exo_shield",
  "drivers/virtio-block", "drivers/virtio-net", "drivers/virtio-console",
  "libs/exo-types", "libs/exo-ipc", "libs/exo-syscall",
]
[workspace.dependencies]
exo-types   = { path = "libs/exo-types" }
exo-ipc     = { path = "libs/exo-ipc" }
exo-syscall = { path = "libs/exo-syscall" }

⚠ Chaque Cargo.toml de server ou driver DOIT déclarer [profile.dev] panic = 'abort' et [profile.release] panic = 'abort'. no_std obligatoire partout.

Template Cargo.toml server/driver
[package]
name = "exo-<nom>"
version = "0.1.0"
edition = "2021"
[[bin]]
name = "<nom>"
path = "src/main.rs"
[dependencies]
exo-types   = { workspace = true }
exo-ipc     = { workspace = true }
exo-syscall = { workspace = true }
[profile.dev]
panic = "abort"
[profile.release]
panic = "abort"

CI enforcement ✦ V3 (obligatoire avant merge)
# SRV-02 : blake3/chacha20 uniquement dans crypto_server
grep -rn 'blake3\|chacha20poly1305' servers/ drivers/ libs/ \
  | grep -v 'servers/crypto_server' && echo 'VIOLATION SRV-02' && exit 1

# no_std IPC : pas de types dynamiques dans protocol.rs
grep -rn 'Vec<\|: String\|Box<\|use alloc' libs/exo-types/ \
  servers/*/src/protocol.rs && echo 'VIOLATION IPC no_std' && exit 1

# panic=abort dans chaque Cargo.toml
for f in servers/*/Cargo.toml drivers/*/Cargo.toml; do
  grep -q 'panic = "abort"' "$f" || { echo "MISSING: $f"; exit 1; }
done


7. Ordre de création — à donner à GPT-5.3-Codex
Cet ordre est la seule table de création valide. device_server PRÉCÈDE tous les drivers.
⚠ Étapes 8 et 9 V1 INVERSÉES. Ordre correct : device_server étape 8, virtio-block étape 9.

Étape	Crate	Condition de démarrage	Validation
1	libs/exo-types	Rien	cargo check — aucune dépendance blake3/chacha20, FixedString présent
2	libs/exo-ipc + exo-syscall	exo-types disponible	cargo check — SpscRing align(64) présent, phoenix.rs compilé
3	servers/ipc_broker	libs complètes	Démarre, écoute les registrations, persistence.rs actif
4	servers/memory_server	ipc_broker disponible	Alloc/Free Ring 3 fonctionnel
5	servers/init_server	ipc_broker + memory_server	PID 1 démarre services, sigchld_handler opérationnel
6	servers/vfs_server	ExoFS monté (kernel Phase 4)	open('/') fonctionne avec PathBuf
7	servers/crypto_server	vfs_server disponible	GenRandom + Hash(data)→ObjectId valide, entropy.rs initialisé
8	servers/device_server ✦	ipc_broker + memory_server	Probe PCI + claim_validator opérationnel
9	drivers/virtio-block ✦	device_server disponible	ExoFS lit/écrit sur disque QEMU
10	drivers/virtio-net + console	device_server disponible	Réseau + console QEMU opérationnels
11	servers/network_server	virtio-net disponible	TCP/UDP fonctionnels
12	servers/exo_shield	Phase 3 ExoPhoenix stable	PrepareIsolation flow complet : broadcast → ack → phoenix_notify(521)


8. Règles absolues — NE JAMAIS VIOLER
Règle	Identifiant	Description
SRV-01	SIGCHLD handler	init_server reçoit le message IPC ChildDied du kernel — sigchld_handler.rs → supervisor.rs. Pas de zombies.
SRV-02	Pas de blake3 hors crypto_server	Aucun crate sauf crypto_server n'importe blake3 ou chacha20poly1305. Inclut les libs partagées. CI grep obligatoire.
SRV-04	Crypto centralisée	Toute opération crypto passe par crypto_server via IPC. ObjectId calculé uniquement dans crypto_server/hash.rs.
IPC-01	CachePadded SpscRing	SpscRing : #[repr(C, align(64))] sur head et tail — implémenté manuellement, pas de crossbeam.
IPC-02 ✦	Types IPC Sized	Tous les types dans protocol.rs sont Sized et à taille fixe. Utiliser FixedString<N>. Aucun &str, Vec<>, String, Box<>.
IPC-03 ✦	sender_pid obligatoire	IpcMessage.sender_pid: u32 renseigné par le kernel. Utilisé pour tous les replies directs.
CAP-01	verify_cap_token au démarrage	Chaque server appelle verify_cap_token() en première instruction de main.rs — panic! si invalide.
CAP-02 ✦	Claim PCI sécurisé	Claim { device_id, driver_cap, nonce } — nonce généré par kernel dans CapToken, non forgeable par le driver.
PHX-01 ✦	PrepareIsolationAck	Chaque server critique retourne PrepareIsolationAck { server, checkpoint_id } avant gel ExoPhoenix.
PHX-02	no_std + panic=abort	#![no_std] et panic='abort' dans chaque crate server/driver. Pas de stack unwinding.

ExoOS — Arborescence Servers + Drivers  ·  Version 3  ·  Mars 2026
