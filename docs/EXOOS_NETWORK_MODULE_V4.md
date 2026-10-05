# ExoOS — Module Réseau `network_server` v4.0
> Architecture complète · Mai 2026  
> Claude Alpha — V4 de zéro, post-cycle beta/gamma (V1→V3.2)  
> Fondée sur lecture directe du dépôt + vérification web smoltcp 0.12

---

## 0. Pourquoi V4 de zéro

Les versions V1–V3.2 ont accumulé des corrections correctes en isolation mais des contradictions structurelles en composition. Deux problèmes de fond irrésolus :

**Problème fondamental 1 — `SpscRing<u16>` n'existe pas.**  
`SpscRing` est un type concret non-générique (kernel/src/ipc/ring/spsc.rs). Il transporte des `RingSlot` (payload = 240 bytes, `RING_SIZE = 16` slots, taille totale = 5 248 bytes). Le type paramétré `SpscRing<u16>` présent depuis V3.0 ne compile pas.

**Problème fondamental 2 — Le SHM inter-process n'est pas wirable dans ExoOS.**  
`SYS_SHMGET`/`SYS_SHMAT` sont déclarés dans `numbers.rs` mais **sans handler dans `table.rs`**. `memory_server` a `SHM_CREATE`/`SHM_ATTACH` qui retournent un handle opaque (`u64`), pas une adresse virtuelle. `SYS_MMAP` est fd-based (pas handle-based). Il n'existe pas de chemin stable pour partager une page entre deux processus Ring 1 sans toucher le kernel stable.

**Conséquence directe** : toute solution nécessitant une mémoire partagée (`U16Ring`, `released_ring` SHM, `DriverInitMsg.released_ring_phys`) est non implémentable sans modification du code stable.

**Solution adoptée en V4** : supprimer toute mémoire partagée inter-process pour le released_ring. Utiliser **l'IPC ExoOS natif** — le seul mécanisme inter-process entièrement opérationnel. C'est cohérent avec l'architecture ExoOS : vfs_server, crypto_server et memory_server communiquent tous par IPC, jamais par SHM brute.

---

## 1. Faits empiriques repo (base de V4)

| Constante | Valeur | Source |
|---|---|---|
| `RING_SIZE` | 16 slots | `kernel/src/ipc/core/constants.rs:38` |
| `MAX_MSG_SIZE` | 240 bytes | `constants.rs:20` |
| `RING_SLOT_SIZE` | 256 bytes (16 + 240) | `constants.rs:28` |
| `SlotCell` taille | 320 bytes (264 raw → align 64) | `ring/slot.rs` + calcul |
| `SpscRing` taille | 5 248 bytes (64+64+16×320) | calculé |
| `IpcMessage` taille | 64 bytes | `ipc_msg.rs:70` assert |
| `IpcMessage.payload` | 48 bytes à offset 16 | `ipc_msg.rs:19, 74` |
| `SYS_DMA_ALLOC` | retourne `(virt, iova)` | `table.rs:2683` |
| `SYS_SHMGET`/`SHMAT` | déclarés, **sans handler** | `numbers.rs` + `table.rs` |
| `memory_server SHM` | handle opaque, pas virt | `mmap_service.rs:325` |
| `TypedChannel<T>` | générique sur SpscRing, max 512 bytes | `ipc/channel/typed.rs` |
| `smoltcp 0.12` `SocketHandle` | dans `smoltcp::iface` | docs.rs confirmé |
| `smoltcp 0.12` `RxToken::consume` | `FnOnce(&mut [u8]) -> R` | docs.rs confirmé |
| `smoltcp 0.12` `Interface::poll` | `(ts, &mut device, &mut sockets)` | docs.rs confirmé |

---

## 2. Périmètre

### 2.1 Réécriture complète (non référencé par kernel actif)

```
servers/network_server/src/      ← réécriture complète
drivers/network/virtio_net/      ← net.rs = 0 lignes → implémentation
drivers/network/loopback/        ← main.rs = 0 lignes → implémentation
```

### 2.2 Modifications minimales du code stable (consensus IA requis)

| Ref | Fichier | Changement | Lignes |
|---|---|---|---|
| **MOD-01** | `Cargo.toml` workspace | `smoltcp = "0.12"` + features | ~8 |
| **MOD-02** | `kernel/src/syscall/table.rs` | 15 handlers BSD → `net_bridge::*` | ~80 |
| **MOD-03** | `kernel/src/syscall/mod.rs` | `pub mod net_bridge;` | ~15 |
| **MOD-04** | `kernel/src/lib.rs` | `net_bridge_preinit()` | 3 |

**`SYSCALL_TABLE_SIZE` reste 547** — SYS 41–55 déjà dans `numbers.rs`, pointent vers `sys_enosys`.

### 2.3 Code non touché

```
kernel/src/exophoenix/   kernel/src/fs/exofs/     kernel/src/scheduler/
kernel/src/security/     servers/crypto_server/   servers/vfs_server/
servers/exo_shield/      kernel/src/ipc/ring/     kernel/src/ipc/core/
```

---

## 3. Architecture V4

```
Ring 3 (Applications POSIX)
        │  socket()/connect()/bind()/send()/recv()/getpeername()...
        │  15 syscalls BSD (SYS 41–55)
        ▼
┌──────────────────────────────────────────────────────────────────────┐
│  kernel/src/syscall/                                                 │
│  table.rs → 15 handlers BSD → net_bridge.rs                        │
│  net_bridge.rs : NET_READY guard + lazy IpcEndpoint lookup          │
└──────────────────────────────┬───────────────────────────────────────┘
                               │  IpcMessage (64 bytes, payload 48 bytes)
                               │  SYS_IPC_SEND/RECV (existants)
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│  servers/network_server/  (Ring 1)                                  │
│                                                                      │
│  ┌──────────────────┐  ┌──────────────────────────────────────────┐ │
│  │  SocketTable[64] │  │  TcpStateStore (~400 Ko)                 │ │
│  │  SocketHandle ∈  │  │  static mut TCP_STATE_STORE             │ │
│  │  smoltcp::iface  │  └──────────────────────────────────────────┘ │
│  └────────┬─────────┘                                               │
│           │                                                          │
│  ┌────────▼───────────────────────────────────────────────────────┐ │
│  │  SmoltcpIface                                                  │ │
│  │  Interface + SocketSet<'static>                                │ │
│  │  ExoNetDevice impl phy::Device (smoltcp 0.12)                 │ │
│  │  static TCP_RX_BUFS / TCP_TX_BUFS / SOCKET_STORAGE           │ │
│  │  poll_ingress_single() — 1 paquet par tick (bounded)          │ │
│  └────────┬───────────────────────────────────────────────────────┘ │
│           │                                                          │
│  ┌────────▼───────────────────────────────────────────────────────┐ │
│  │  NetBufPool (DMA allouée par network_server via SYS_DMA_ALLOC) │ │
│  │  RX: 256 pages × 4096 = 1 Mo  (virt: lecture payload CPU)     │ │
│  │  TX: 256 pages × 4096 = 1 Mo  (virt: écriture payload CPU)    │ │
│  │  IOVA RX/TX → envoyé à virtio_net via DriverInitMsg (IPC)     │ │
│  │                                                                │ │
│  │  released_buf: [u16; 64]  ← pool_idx libérés ce tick         │ │
│  │  released_count: usize                                         │ │
│  └────────┬───────────────────────────────────────────────────────┘ │
└───────────┼──────────────────────────────────────────────────────────┘
            │  IPC bidirectionnel (IpcMessage 64 bytes)
            │  ┌─ DriverInitMsg (network_server → virtio_net, 1 fois)
            │  └─ RxReleaseMsg  (network_server → virtio_net, par tick)
            ▼
┌──────────────────────────────────────────────────────────────────────┐
│  drivers/network/virtio_net/  (Ring 1)                              │
│                                                                      │
│  vring RX ←─ DMA (IOVA) ←─ NIC (QEMU virtio-net)                  │
│  vring TX ──→ DMA (IOVA) ──→ NIC                                   │
│                                                                      │
│  rx_submitted: [bool; 256]  ← slot soumis au vring                 │
│  refill via RxReleaseMsg reçus depuis network_server               │
│  Mode IRQ → POLL (seuil 32)                                        │
└──────────────────────────────────────────────────────────────────────┘
```

### 3.1 Protocole IPC réseau — deux canaux

**Canal A — kernel → network_server** (existant, SYS_IPC_SEND/RECV) :  
Syscalls BSD applicatifs. `NetMsg` (48 bytes) dans `IpcMessage.payload`.

**Canal B — network_server → virtio_net** (nouveau, même mécanisme IPC) :  
`DriverInitMsg` (envoi unique au démarrage) + `RxReleaseMsg` (par tick de poll).  
Aucune mémoire partagée. Aucune SHM. IPC pur.

---

## 4. `net_bridge.rs` — Interface kernel ↔ network_server

```rust
// Garde d'initialisation — positionné par net_bridge_preinit() dans lib.rs
static NET_READY: AtomicBool = AtomicBool::new(false);

/// MOD-04 : appelé depuis lib.rs après fs_bridge_init().
/// NE fait PAS le lookup (network_server n'est pas encore démarré).
pub unsafe fn net_bridge_preinit() {
    NET_READY.store(true, Ordering::Release);
}

// Lazy lookup — résout la race boot de V2
static NET_SERVER_EP: Mutex<Option<IpcEndpoint>> = Mutex::new(None);

fn ensure_net_server() -> Option<IpcEndpoint> {
    let mut ep = NET_SERVER_EP.lock();
    if ep.is_none() {
        *ep = ipc_broker::lookup("network_server");
    }
    *ep
}

pub enum NetBridgeError {
    NotReady,   // → ENOSYS (-38)
    NoServer,   // → ENETDOWN (-100)
    BadFd,      // → EBADF (-9)
    BadAddr,    // → EFAULT (-14)
    Io(i64),
}

pub fn bridge_result(r: Result<i64, NetBridgeError>) -> i64 {
    match r {
        Ok(v)                         => v,
        Err(NetBridgeError::NotReady) => -38,
        Err(NetBridgeError::NoServer) => -100,
        Err(NetBridgeError::BadFd)    => -9,
        Err(NetBridgeError::BadAddr)  => -14,
        Err(NetBridgeError::Io(e))    => e,
    }
}

// 15 fonctions (14 BSD + getpeername)
pub fn net_socket(domain: u32, kind: u32, protocol: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_connect(fd: u32, addr_ptr: u64, addrlen: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_bind(fd: u32, addr_ptr: u64, addrlen: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_listen(fd: u32, backlog: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_accept(fd: u32, addr_ptr: u64, addrlen_ptr: u64, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_sendto(fd: u32, buf: u64, len: usize, flags: u32, addr_ptr: u64, addrlen: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_recvfrom(fd: u32, buf: u64, len: usize, flags: u32, addr_ptr: u64, addrlen_ptr: u64, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_sendmsg(fd: u32, msg_ptr: u64, flags: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_recvmsg(fd: u32, msg_ptr: u64, flags: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_shutdown(fd: u32, how: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_getsockname(fd: u32, addr_ptr: u64, addrlen_ptr: u64, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_getpeername(fd: u32, addr_ptr: u64, addrlen_ptr: u64, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_socketpair(domain: u32, kind: u32, protocol: u32, fds_ptr: u64, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_setsockopt(fd: u32, level: u32, optname: u32, optval: u64, optlen: u32, pid: u32) -> Result<i64, NetBridgeError>
pub fn net_getsockopt(fd: u32, level: u32, optname: u32, optval: u64, optlen_ptr: u64, pid: u32) -> Result<i64, NetBridgeError>
```

---

## 5. `network_server` — Réécriture complète

### 5.1 Arborescence

```
servers/network_server/
├── Cargo.toml              # no_std, panic=abort, smoltcp 0.12
└── src/
    ├── main.rs             # CAP-01 → register → init → boucle
    ├── protocol.rs         # NetMsg(48B), NetReply(48B), CtrlMsg, opcodes
    ├── socket_table.rs     # SocketTable[64], SocketHandle ∈ smoltcp::iface
    ├── tcp_store.rs        # TcpStateStore (~400 Ko), const new_empty()
    ├── buf_pool.rs         # NetBufPool : SYS_DMA_ALLOC → (virt, iova)
    ├── virtio_device.rs    # ExoNetDevice impl phy::Device + released_buf
    ├── smoltcp_iface.rs    # SmoltcpIface, statics TCP bufs
    ├── isolation.rs        # Phoenix PrepareIsolation
    └── driver_link.rs      # DriverInitMsg + RxReleaseMsg via IPC
```

### 5.2 `protocol.rs`

```rust
// ─── Opcodes BSD (Canal A : kernel → network_server) ────────────────────────
pub const NET_MSG_OPEN:        u32 = 0x4E00;
pub const NET_MSG_CONNECT:     u32 = 0x4E01;
pub const NET_MSG_BIND:        u32 = 0x4E02;
pub const NET_MSG_LISTEN:      u32 = 0x4E03;
pub const NET_MSG_ACCEPT:      u32 = 0x4E04;
pub const NET_MSG_SENDTO:      u32 = 0x4E05;
pub const NET_MSG_RECVFROM:    u32 = 0x4E06;
pub const NET_MSG_SENDMSG:     u32 = 0x4E07;
pub const NET_MSG_RECVMSG:     u32 = 0x4E08;
pub const NET_MSG_SHUTDOWN:    u32 = 0x4E09;
pub const NET_MSG_GETSOCKNAME: u32 = 0x4E0A;
pub const NET_MSG_SOCKETPAIR:  u32 = 0x4E0B;
pub const NET_MSG_SETSOCKOPT:  u32 = 0x4E0C;
pub const NET_MSG_GETSOCKOPT:  u32 = 0x4E0D;
pub const NET_MSG_CLOSE:       u32 = 0x4E0E;
pub const NET_MSG_GETPEERNAME: u32 = 0x4E0F;

// ─── Opcodes contrôle (Canal B : network_server ↔ virtio_net) ───────────────
pub const NET_CTRL_DRIVER_INIT:   u32 = 0x4F00; // network_server → virtio_net (1 fois)
pub const NET_CTRL_RX_RELEASE:    u32 = 0x4F01; // network_server → virtio_net (par tick)
pub const NET_CTRL_MAC_QUERY:     u32 = 0x4F02; // network_server → virtio_net (1 fois)
pub const NET_CTRL_MAC_REPLY:     u32 = 0x4F03; // virtio_net → network_server

/// Requête BSD — 48 bytes, payload IpcMessage.
///
/// Calcul exact repr(C) :
///   opcode(4) + sender_pid(4) + fd(4) + _pad0(4) = 16  [_pad0 aligne arg1 sur 8]
///   arg1(8) + arg2(8)                             = 32
///   arg3(4) + arg4(4)                             = 40
///   _reserved([u8;8])                             = 48  ← total
#[repr(C)]
pub struct NetMsg {
    pub opcode:     u32,
    pub sender_pid: u32,
    pub fd:         u32,
    pub _pad0:      u32,      // alignement arg1 (u64 → offset 16, multiple de 8)
    pub arg1:       u64,
    pub arg2:       u64,
    pub arg3:       u32,
    pub arg4:       u32,
    pub _reserved:  [u8; 8], // réservé Phase 2 ExoSocket
}
const _: () = assert!(core::mem::size_of::<NetMsg>() == 48);

/// Réponse BSD — 48 bytes.
/// Si données > 40 bytes (ex: recvfrom large) : ObjectId SHM dans payload.
#[repr(C)]
pub struct NetReply {
    pub status:  i64,
    pub payload: [u8; 40],
}
const _: () = assert!(core::mem::size_of::<NetReply>() == 48);

/// DriverInitMsg : network_server → virtio_net (envoi unique au démarrage).
///
/// network_server alloue les pages DMA, obtient (virt, iova) via SYS_DMA_ALLOC.
/// virt : addresses virtuelles utilisées par network_server pour lire/écrire payloads.
/// iova : addresses IOMMU utilisées par virtio_net pour les descripteurs vring.
/// Seules les IOVA sont transmises à virtio_net — jamais les adresses virtuelles.
#[repr(C)]
pub struct DriverInitMsg {
    pub opcode:        u32,     // = NET_CTRL_DRIVER_INIT
    pub pool_count:    u32,     // = RX_POOL_SIZE = 256
    pub rx_base_iova:  u64,     // IOVA bloc RX (256 × PAGE_SIZE)
    pub tx_base_iova:  u64,     // IOVA bloc TX (256 × PAGE_SIZE)
    pub hdr_size:      u32,     // 10 (legacy) ou 12 (MRG_RXBUF)
    pub _pad:          u32,
}
const _: () = assert!(core::mem::size_of::<DriverInitMsg>() == 32);

/// RxReleaseMsg : network_server → virtio_net (après chaque tick de poll).
///
/// Contient les pool_idx RX libérés par ExoRxToken::consume() ce tick.
/// count × u16 ≤ 44 bytes → max 22 pool_idx par message.
/// En opération normale (poll_ingress_single = 1 paquet/tick) : count = 1.
#[repr(C)]
pub struct RxReleaseMsg {
    pub opcode:    u32,          // = NET_CTRL_RX_RELEASE
    pub count:     u32,          // nombre de pool_idx valides
    pub pool_idx:  [u16; 22],   // pool_idx libérés (22 × 2 = 44 bytes)
}
const _: () = assert!(core::mem::size_of::<RxReleaseMsg>() == 48);
// 4 + 4 + 44 = 52 > 48 → ajuster
// Correction : 4 + 4 + 20×2 = 4+4+40 = 48 → max 20 pool_idx par message
```

**Correction calcul RxReleaseMsg** :

```rust
#[repr(C)]
pub struct RxReleaseMsg {
    pub opcode:   u32,
    pub count:    u32,
    pub pool_idx: [u16; 20],  // 4+4+40 = 48 bytes ✓
}
const _: () = assert!(core::mem::size_of::<RxReleaseMsg>() == 48);

// Si released_count > 20 : envoyer plusieurs RxReleaseMsg consécutifs.
// Cas extrême théorique (count=256 en 1 tick) = 13 messages IPC.
// En pratique avec poll_ingress_single() : count ≤ 1 par tick.
```

### 5.3 `buf_pool.rs` — NetBufPool

```rust
// network_server ALLOUE les pages DMA via SYS_DMA_ALLOC (534).
// SYS_DMA_ALLOC retourne (virt, iova) :
//   virt : adresse virtuelle dans l'espace network_server → lecture/écriture payloads
//   iova : adresse IOMMU → transmise à virtio_net pour les descripteurs vring

pub const RX_POOL_SIZE: usize = 256;
pub const TX_POOL_SIZE: usize = 256;
pub const VIRTIO_NET_HDR_SIZE_LEGACY: usize = 10;  // sans VIRTIO_NET_F_MRG_RXBUF
pub const VIRTIO_NET_HDR_SIZE_MRGBUF: usize = 12;  // avec VIRTIO_NET_F_MRG_RXBUF

pub struct NetBufPool {
    rx_base_virt: u64,   // virtuel network_server — lecture payload
    rx_base_iova: u64,   // IOMMU — envoyé à virtio_net
    tx_base_virt: u64,
    tx_base_iova: u64,
    hdr_size:     usize, // 10 ou 12 selon négociation features
    rx_alloc:     [AtomicBool; RX_POOL_SIZE],
    tx_alloc:     [AtomicBool; TX_POOL_SIZE],
}

impl NetBufPool {
    /// Initialise le pool DMA.
    /// hdr_size transmis après MAC_QUERY (negotiate_hdr_size dans virtio_net).
    pub fn init(hdr_size: usize) -> Result<Self, i64> {
        // SYS_DMA_ALLOC(534) : alloc + map dans l'espace courant
        let (rx_virt, rx_iova) = syscall::dma_alloc(
            RX_POOL_SIZE * PAGE_SIZE, DMA_DIR_FROM_DEVICE, 0, DMA_PINNED, 0
        )?;
        let (tx_virt, tx_iova) = syscall::dma_alloc(
            TX_POOL_SIZE * PAGE_SIZE, DMA_DIR_TO_DEVICE, 0, DMA_PINNED, 0
        )?;
        Ok(Self {
            rx_base_virt: rx_virt,
            rx_base_iova: rx_iova,
            tx_base_virt: tx_virt,
            tx_base_iova: tx_iova,
            hdr_size,
            rx_alloc: [const { AtomicBool::new(false) }; RX_POOL_SIZE],
            tx_alloc: [const { AtomicBool::new(false) }; TX_POOL_SIZE],
        })
    }

    /// Adresse virtuelle du payload RX (après virtio_net_hdr).
    pub fn rx_payload_ptr_mut(&self, idx: usize) -> *mut u8 {
        (self.rx_base_virt + (idx * PAGE_SIZE + self.hdr_size) as u64) as *mut u8
    }

    /// IOVA du slot RX (début de page, inclut virtio_net_hdr).
    pub fn rx_iova(&self, idx: usize) -> u64 {
        self.rx_base_iova + (idx * PAGE_SIZE) as u64
    }

    pub fn tx_payload_ptr_mut(&self, idx: usize) -> *mut u8 {
        (self.tx_base_virt + (idx * PAGE_SIZE + self.hdr_size) as u64) as *mut u8
    }

    pub fn tx_iova(&self, idx: usize) -> u64 {
        self.tx_base_iova + (idx * PAGE_SIZE) as u64
    }

    pub fn tx_alloc(&self) -> Option<u16> {
        for (i, used) in self.tx_alloc.iter().enumerate() {
            if used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return Some(i as u16);
            }
        }
        None
    }

    pub fn tx_free(&self, idx: u16) {
        self.tx_alloc[idx as usize].store(false, Ordering::Release);
    }

    // Note : rx_alloc n'est PAS utilisé ici.
    // Les slots RX sont libérés par RxReleaseMsg IPC → virtio_net.
    // network_server ne gère plus l'état RX — c'est virtio_net qui sait
    // quels slots sont soumis au vring.
}
```

### 5.4 `virtio_device.rs` — trait `phy::Device` smoltcp 0.12

```rust
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// Adaptateur smoltcp phy::Device pour ExoOS.
/// ExoNetDevice vit dans network_server et lit les paquets depuis le NetBufPool
/// via les pool_idx fournis par virtio_net dans le SpscRing IPC.
pub struct ExoNetDevice {
    // Ring IPC : virtio_net → network_server (paquets RX reçus)
    rx_ring:       *mut SpscRing,  // lit les NetBufRef (pool_idx + len) sérialisés
    // Ring IPC : network_server → virtio_net (paquets TX à envoyer)
    tx_ring:       *mut SpscRing,
    buf_pool:      *const NetBufPool,
    // Accumulateur de pool_idx libérés ce tick → vidé dans driver_link::flush_released()
    pub released_buf:   [u16; 64],
    pub released_count: usize,
}

// Safety : ExoNetDevice utilisé exclusivement dans la boucle principale
// single-threaded de network_server (IPC-03).
unsafe impl Send for ExoNetDevice {}

impl Device for ExoNetDevice {
    type RxToken<'a> = ExoRxToken where Self: 'a;
    type TxToken<'a> = ExoTxToken where Self: 'a;

    /// Reçoit le prochain paquet RX depuis le SpscRing IPC (virtio_net → ns).
    ///
    /// Le SpscRing transporte des RingSlot dont le payload encode :
    ///   [0..2] pool_idx : u16 (le_bytes)
    ///   [2..4] len      : u16 (le_bytes)
    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let pool = unsafe { &*self.buf_pool };
        let rx_ring = unsafe { &mut *self.rx_ring };

        // Lire un NetBufRef sérialisé depuis le SpscRing
        let mut buf = [0u8; 4];
        rx_ring.pop_into(&mut buf).ok()?;
        let rx_idx = u16::from_le_bytes([buf[0], buf[1]]);
        let rx_len = u16::from_le_bytes([buf[2], buf[3]]);

        // Réserver un slot TX pour l'ACK potentiel
        let tx_idx = match pool.tx_alloc() {
            Some(idx) => idx,
            None => {
                // TX saturé : libérer le slot RX immédiatement via released_buf
                // (pas de corruption — le paquet n'a pas été lu par smoltcp)
                if self.released_count < self.released_buf.len() {
                    self.released_buf[self.released_count] = rx_idx;
                    self.released_count += 1;
                }
                // else : released_buf plein → le pool_idx est perdu
                // Invariant de sécurité : released_buf[64] > RING_SIZE[16],
                // en pratique impossible d'en avoir autant en simultané.
                return None;
            }
        };

        Some((
            ExoRxToken {
                pool_idx: rx_idx,
                len:      rx_len as usize,
                pool:     self.buf_pool,
                released_buf:   &mut self.released_buf as *mut [u16; 64],
                released_count: &mut self.released_count as *mut usize,
            },
            ExoTxToken {
                pool_idx: tx_idx,
                tx_ring:  self.tx_ring,
                pool:     self.buf_pool,
            },
        ))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        let pool = unsafe { &*self.buf_pool };
        let idx = pool.tx_alloc()?;
        Some(ExoTxToken { pool_idx: idx, tx_ring: self.tx_ring, pool: self.buf_pool })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(32);
        caps
    }
}

pub struct ExoRxToken {
    pool_idx:      u16,
    len:           usize,
    pool:          *const NetBufPool,
    released_buf:  *mut [u16; 64],   // ptr vers ExoNetDevice.released_buf
    released_count: *mut usize,
}

impl RxToken for ExoRxToken {
    /// smoltcp 0.12 : FnOnce(&mut [u8]) -> R (pas &[u8])
    fn consume<R, F>(self, f: F) -> R
    where F: FnOnce(&mut [u8]) -> R
    {
        let pool = unsafe { &*self.pool };
        // slice sur le payload (après virtio_net_hdr)
        let ptr = pool.rx_payload_ptr_mut(self.pool_idx as usize);
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, self.len) };

        let result = f(slice); // smoltcp lit le paquet ICI

        // APRÈS lecture complète : enregistrer dans released_buf
        // Le pool_idx sera envoyé à virtio_net via RxReleaseMsg à la fin du tick.
        // C'est ici que la race RACE-RX est éliminée : virtio_net ne récupère
        // le slot QUE après ce signal IPC, jamais avant.
        let count = unsafe { &mut *self.released_count };
        let rbuf  = unsafe { &mut *self.released_buf };
        if *count < rbuf.len() {
            rbuf[*count] = self.pool_idx;
            *count += 1;
        }

        result
    }
}

pub struct ExoTxToken {
    pool_idx: u16,
    tx_ring:  *mut SpscRing,
    pool:     *const NetBufPool,
}

impl TxToken for ExoTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where F: FnOnce(&mut [u8]) -> R
    {
        let pool = unsafe { &*self.pool };
        let ptr   = pool.tx_payload_ptr_mut(self.pool_idx as usize);
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, len) };

        let result = f(slice); // smoltcp écrit le paquet ICI

        // Écrire virtio_net_hdr (zeros = no checksum, no GSO)
        let hdr_ptr = (pool.tx_base_virt()
                       + self.pool_idx as u64 * PAGE_SIZE as u64) as *mut u8;
        unsafe { core::ptr::write_bytes(hdr_ptr, 0, pool.hdr_size()); }

        // Sérialiser NetBufRef et pousser dans le SpscRing TX
        let tx_ring = unsafe { &mut *self.tx_ring };
        let buf: [u8; 4] = [
            (self.pool_idx & 0xFF) as u8,
            (self.pool_idx >> 8)   as u8,
            (len & 0xFF)           as u8,
            ((len >> 8) & 0xFF)    as u8,
        ];
        let _ = tx_ring.push_copy(&buf, MsgFlags::default());

        result
    }
}
```

### 5.5 `smoltcp_iface.rs` — smoltcp 0.12

```rust
use smoltcp::iface::{Config, Interface, SocketSet, SocketHandle};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address};

// Buffers TCP en .bss — lifetime 'static, contournement SocketSet<'static>
static mut TCP_RX_BUFS: [[u8; 4096]; MAX_SOCKETS] = [[0u8; 4096]; MAX_SOCKETS];
static mut TCP_TX_BUFS: [[u8; 4096]; MAX_SOCKETS] = [[0u8; 4096]; MAX_SOCKETS];
static mut SOCKET_STORAGE: [smoltcp::iface::SocketStorage<'static>; MAX_SOCKETS] =
    [smoltcp::iface::SocketStorage::EMPTY; MAX_SOCKETS];

pub struct SmoltcpIface {
    iface:   Interface,
    sockets: SocketSet<'static>,
    // ExoNetDevice stocké séparément dans NetworkService — passé à chaque poll
}

impl SmoltcpIface {
    /// Init unique. Appelé après réception de MAC_REPLY depuis virtio_net.
    /// Safety : appelé une seule fois avant démarrage de la boucle.
    pub unsafe fn init(mac: EthernetAddress, ip: Ipv4Address, prefix_len: u8,
                       device: &mut ExoNetDevice, now: Instant) -> Self
    {
        let config = Config::new(mac.into());
        // Interface::new() appelle device.capabilities() ici →
        // ExoNetDevice retourne MTU=1514, burst=32 (fix BUG-V3-04)
        let mut iface = Interface::new(config, device, now);
        iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::new(IpAddress::Ipv4(ip), prefix_len)).ok();
        });
        let sockets = SocketSet::new(&mut SOCKET_STORAGE[..]);
        Self { iface, sockets }
    }

    /// Alloue un socket TCP avec buffers statiques (fix BUG-V3-01 + BUG-V3-02).
    pub fn alloc_tcp_socket(&mut self, slot: usize) -> SocketHandle {
        // Safety : slot utilisé une seule fois (garanti par SocketTable)
        let rx_buf = tcp::SocketBuffer::new(unsafe { &mut TCP_RX_BUFS[slot][..] });
        let tx_buf = tcp::SocketBuffer::new(unsafe { &mut TCP_TX_BUFS[slot][..] });
        self.sockets.add(tcp::Socket::new(rx_buf, tx_buf))
    }

    /// poll_ingress_single() : 1 paquet entrant par appel (bounded work — PHX-02).
    /// smoltcp::Interface::poll() est non borné → violerait le scheduler coopératif.
    pub fn poll_one(&mut self, now: Instant, device: &mut ExoNetDevice) -> bool {
        self.iface.poll_ingress_single(now, device, &mut self.sockets)
    }

    /// Flush tous les paquets sortants (borné par le TX ring = 256 slots).
    pub fn poll_egress(&mut self, now: Instant, device: &mut ExoNetDevice) {
        self.iface.poll_egress(now, device, &mut self.sockets);
    }

    /// Drain complet pour Phoenix PrepareIsolation.
    pub fn drain_all(&mut self, now: Instant, device: &mut ExoNetDevice) {
        self.iface.poll(now, device, &mut self.sockets);
    }
}
```

### 5.6 `driver_link.rs` — IPC network_server ↔ virtio_net

```rust
use crate::protocol::{DriverInitMsg, RxReleaseMsg, NET_CTRL_DRIVER_INIT,
                       NET_CTRL_RX_RELEASE, NET_CTRL_MAC_QUERY, NET_CTRL_MAC_REPLY};

pub struct DriverLink {
    pub virtio_ep:    IpcEndpoint,
    pub rx_ring_ipc:  *mut SpscRing,   // SpscRing IPC virtio_net → network_server (RX paquets)
    pub tx_ring_ipc:  *mut SpscRing,   // SpscRing IPC network_server → virtio_net (TX paquets)
}

/// Connexion au virtio_net : envoi DriverInitMsg + réception MAC_REPLY.
pub fn connect_virtio_net(buf_pool: &NetBufPool) -> Result<(DriverLink, EthernetAddress), i64> {
    // Lookup virtio_net avec backoff (max 10 × 100 ms)
    let ep = wait_for_endpoint("virtio_net", 10)?;

    // Envoyer DriverInitMsg : transmet les IOVA (pas les virt) au driver
    let init_msg = DriverInitMsg {
        opcode:       NET_CTRL_DRIVER_INIT,
        pool_count:   RX_POOL_SIZE as u32,
        rx_base_iova: buf_pool.rx_base_iova(),
        tx_base_iova: buf_pool.tx_base_iova(),
        hdr_size:     VIRTIO_NET_HDR_SIZE_LEGACY as u32, // 10, affiné après MAC_QUERY
        _pad:         0,
    };
    let mut payload = [0u8; 48];
    payload[..32].copy_from_slice(unsafe {
        core::slice::from_raw_parts(&init_msg as *const _ as *const u8, 32)
    });
    ipc_send_raw(ep, NET_CTRL_DRIVER_INIT, &payload)?;

    // Envoyer MAC_QUERY + recevoir MAC_REPLY + hdr_size réel
    ipc_send_opcode(ep, NET_CTRL_MAC_QUERY)?;
    let reply = ipc_recv_from(ep)?;
    let mac = EthernetAddress::from_bytes(&reply.payload[0..6]);
    let hdr_size = u32::from_le_bytes(reply.payload[8..12].try_into().unwrap()) as usize;

    // Obtenir les ring IPC créés par virtio_net (échange de pointeurs via IPC)
    // virtio_net alloue ses SpscRing dans son propre espace et envoie
    // les adresses des rings via IPC — network_server les mappe via SYS_MMAP
    // sur une page DMA partagée allouée par virtio_net.
    // Simplification Phase 1 : utiliser le mécanisme SpscRing du kernel IPC
    // (ipc_broker crée les canaux à la connexion, retourne les endpoints).
    let rx_ring = ipc_broker::get_spsc_ring(ep, IPC_ROLE_RX)?;
    let tx_ring = ipc_broker::get_spsc_ring(ep, IPC_ROLE_TX)?;

    Ok((DriverLink { virtio_ep: ep, rx_ring_ipc: rx_ring, tx_ring_ipc: tx_ring },
        mac))
}

/// Flusher les pool_idx libérés vers virtio_net (appelé après chaque tick de poll).
///
/// Envoie des RxReleaseMsg IPC contenant les pool_idx accumulés dans
/// ExoNetDevice.released_buf depuis le dernier flush.
pub fn flush_released(link: &DriverLink, device: &mut ExoNetDevice) {
    if device.released_count == 0 {
        return;
    }

    let mut sent = 0usize;
    while sent < device.released_count {
        let batch = &device.released_buf[sent..device.released_count];
        let count = batch.len().min(20); // max 20 par RxReleaseMsg

        let mut msg = RxReleaseMsg {
            opcode: NET_CTRL_RX_RELEASE,
            count:  count as u32,
            pool_idx: [0u16; 20],
        };
        msg.pool_idx[..count].copy_from_slice(&batch[..count]);

        let mut payload = [0u8; 48];
        payload[..48].copy_from_slice(unsafe {
            core::slice::from_raw_parts(&msg as *const _ as *const u8, 48)
        });
        let _ = ipc_send_raw(link.virtio_ep, NET_CTRL_RX_RELEASE, &payload);
        sent += count;
    }

    device.released_count = 0;
}
```

### 5.7 `tcp_store.rs` — Phoenix (fix BUG-V3-03)

```rust
pub const MAX_SOCKETS: usize = 64;

// Taille exacte TcpSocketState avec #[repr(C)] :
// local_addr(4) + local_port(2) + _pad0(2) + remote_addr(4) + remote_port(2)
// + state(1) + _pad1(1) + rx_len(2) + tx_len(2) + _pad2(2) + seq_num(4) + ack_num(4)
// + rx_buf(2048) + tx_buf(4096)
// = 32 + 6144 = 6176 bytes
pub const TCP_SOCKET_STATE_SIZE: usize = 6176;
pub const STORE_SERIALIZED_SIZE: usize = MAX_SOCKETS * TCP_SOCKET_STATE_SIZE; // 395_264

#[repr(C)]
pub struct TcpSocketState {
    pub local_addr:  u32,
    pub local_port:  u16, pub _pad0: u16,
    pub remote_addr: u32,
    pub remote_port: u16,
    pub state:       u8,  pub _pad1: u8,
    pub rx_len:      u16,
    pub tx_len:      u16, pub _pad2: u16,
    pub seq_num:     u32,
    pub ack_num:     u32,
    pub rx_buf:      [u8; 2048],
    pub tx_buf:      [u8; 4096],
}
const _: () = assert!(core::mem::size_of::<TcpSocketState>() == TCP_SOCKET_STATE_SIZE);

pub struct TcpStateStore {
    slots: [Option<TcpSocketState>; MAX_SOCKETS],
    count: usize,
}

impl TcpStateStore {
    // const fn → utilisable dans `static mut TCP_STATE_STORE` (fix BUG-V3-03)
    pub const fn new_empty() -> Self {
        Self { slots: [const { None }; MAX_SOCKETS], count: 0 }
    }

    pub fn serialize_into(&self, dst: &mut [u8; STORE_SERIALIZED_SIZE]) { /* ... */ }
    pub fn restore_from(&mut self, src: &[u8; STORE_SERIALIZED_SIZE]) { /* ... */ }
}
```

### 5.8 `main.rs` — Boucle principale (fixes BUG-V3-03 + BUG-V3-04)

```rust
// Statics → .bss, zéro stack large (fix BUG-V3-03)
static mut NETWORK_SERVICE: Option<NetworkService> = None;
static mut TCP_STATE_STORE: TcpStateStore = TcpStateStore::new_empty();

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // CAP-01 : première instruction absolue
    verify_cap_token(cap_token_from_env());

    // SRV-01
    ipc_broker::register("network_server");

    // Négocier hdr_size avec virtio_net (MAC_QUERY initial)
    // Phase 1 : défaut 10 bytes (legacy), affiné dans connect_virtio_net()
    let hdr_size = VIRTIO_NET_HDR_SIZE_LEGACY;

    let buf_pool = match NetBufPool::init(hdr_size) {
        Ok(p) => p,
        Err(e) => { log_fatal("DMA alloc", e); syscall::exit(-1); }
    };

    let (driver, mac) = match driver_link::connect_virtio_net(&buf_pool) {
        Ok(d) => d,
        Err(e) => { log_fatal("virtio_net connect", e); syscall::exit(-1); }
    };

    let mut device = ExoNetDevice {
        rx_ring:       driver.rx_ring_ipc,
        tx_ring:       driver.tx_ring_ipc,
        buf_pool:      &buf_pool as *const _,
        released_buf:  [0u16; 64],
        released_count: 0,
    };

    let ip = Ipv4Address::new(10, 0, 2, 15); // Phase 2 : DHCP

    // SmoltcpIface::init() reçoit le vrai device → fix BUG-V3-04
    let smoltcp = unsafe { SmoltcpIface::init(mac, ip, 24, &mut device, now()) };

    unsafe {
        NETWORK_SERVICE = Some(NetworkService {
            buf_pool,
            driver,
            smoltcp,
            tcp_store: &mut TCP_STATE_STORE,
            phoenix_ep: ipc_broker::lookup("exo_shield"),
        });
    }

    // Hoist avant boucle : fix POINT-3 (gamma)
    let service = unsafe { NETWORK_SERVICE.as_mut().unwrap_unchecked() };

    loop {
        // 1. Traiter jusqu'à 16 requêtes IPC applicatives (Canal A)
        let mut ipc_budget = 16u32;
        while ipc_budget > 0 {
            match recv_ipc_nonblocking() {
                Some((msg, sender)) => {
                    let reply = service.dispatch(&msg, sender);
                    send_ipc_reply(sender, reply);
                    ipc_budget -= 1;
                }
                None => break,
            }
        }

        // 2. Traiter 1 paquet ingress (bounded — PHX-02)
        let now = Instant::from_millis(current_time_ms());
        service.smoltcp.poll_one(now, &mut device);

        // 3. Flush TX
        service.smoltcp.poll_egress(now, &mut device);

        // 4. Envoyer les pool_idx libérés à virtio_net (Canal B — fix RACE-RX)
        driver_link::flush_released(&service.driver, &mut device);

        // 5. Yield coopératif (PHX-02)
        syscall::sched_yield();
    }
}
```

---

## 6. `virtio_net` — Driver (réécriture net.rs)

```rust
pub const POLL_THRESHOLD: usize = 32;
pub const RX_POOL_SIZE:   usize = 256;

pub struct VirtioNet {
    rx_vring:     VirtQueue,
    tx_vring:     VirtQueue,
    // IOVA RX/TX : reçues depuis network_server via DriverInitMsg
    rx_base_iova: u64,
    tx_base_iova: u64,
    hdr_size:     usize,
    // SpscRing IPC kernel : réseau → virtio_net (RX paquets sortants vers ns)
    rx_ring:      SpscRing,  // virtio_net → network_server
    tx_ring:      SpscRing,  // network_server → virtio_net
    mode:         TransferMode,
    // Suivi des slots soumis au vring (côté virtio_net uniquement)
    rx_submitted: [bool; RX_POOL_SIZE],
}

pub fn populate_rx_descriptors(&mut self) {
    for i in 0..RX_POOL_SIZE {
        let iova = self.rx_base_iova + (i * PAGE_SIZE) as u64;
        self.rx_vring.add_descriptor(iova, PAGE_SIZE as u32, VRING_DESC_F_WRITE);
        self.rx_submitted[i] = true;
    }
    self.rx_vring.notify();
}

/// Drain du used ring RX + push des NetBufRef dans le SpscRing IPC.
/// Les pool_idx ne sont PAS marqués libres ici — c'est network_server
/// qui envoie RxReleaseMsg après lecture par smoltcp (fix RACE-RX).
pub fn handle_rx_irq(&mut self) -> usize {
    let mut count = 0;
    while let Some((desc_idx, len)) = self.rx_vring.pop_used() {
        let pool_idx = self.desc_to_pool_idx(desc_idx);
        self.rx_submitted[pool_idx] = false; // slot retiré du vring

        let payload_len = len.saturating_sub(self.hdr_size as u32) as u16;

        // Sérialiser NetBufRef dans RingSlot.payload[0..4]
        let buf: [u8; 4] = [
            (pool_idx & 0xFF) as u8, (pool_idx >> 8) as u8,
            (payload_len & 0xFF) as u8, (payload_len >> 8) as u8,
        ];
        let _ = self.rx_ring.push_copy(&buf, MsgFlags::default());
        count += 1;
    }
    if count >= POLL_THRESHOLD {
        self.mode = TransferMode::Poll;
        self.mask_irq();
    }
    count
}

/// Refill : traiter les RxReleaseMsg IPC reçus depuis network_server.
/// Appelé en début de boucle, avant handle_rx_irq().
///
/// Ici seulement, après confirmation explicite de network_server que smoltcp
/// a fini de lire, le slot est re-soumis au vring (fix RACE-RX).
pub fn process_rx_releases(&mut self) {
    let mut refilled = 0;
    let mut buf = [0u8; 48];

    // Drainer les RxReleaseMsg du tx_ring (network_server → virtio_net)
    while self.tx_ring.pop_into(&mut buf).is_ok() {
        let opcode = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if opcode != NET_CTRL_RX_RELEASE {
            continue; // gérer DriverInitMsg et autres opcodes séparément
        }
        let count = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
        for i in 0..count.min(20) {
            let pool_idx = u16::from_le_bytes(buf[8 + i*2..10 + i*2].try_into().unwrap());
            if !self.rx_submitted[pool_idx as usize] {
                let iova = self.rx_base_iova + (pool_idx as usize * PAGE_SIZE) as u64;
                self.rx_vring.add_descriptor(iova, PAGE_SIZE as u32, VRING_DESC_F_WRITE);
                self.rx_submitted[pool_idx as usize] = true;
                refilled += 1;
            }
        }
    }
    if refilled > 0 {
        self.rx_vring.notify();
    }
}

pub fn flush_tx(&mut self) {
    let mut buf = [0u8; 4];
    // Drainer les TX soumis par network_server (sérialisés dans rx_ring IPC côté ns)
    // Note : les noms rx/tx du SpscRing sont du point de vue de network_server.
    // Du point de vue virtio_net : on lit le ring "tx_ns" pour trouver les paquets à envoyer.
    while self.rx_ring.pop_into(&mut buf).is_ok() {
        let pool_idx  = u16::from_le_bytes([buf[0], buf[1]]);
        let payload_len = u16::from_le_bytes([buf[2], buf[3]]);
        let total_len = self.hdr_size + payload_len as usize;
        let iova = self.tx_base_iova + (pool_idx as usize * PAGE_SIZE) as u64;
        self.tx_vring.add_descriptor(iova, total_len as u32, 0);
    }
    self.tx_vring.notify();
}

/// Taille du header virtio-net selon les features négociées (ALPHA-NEW-03).
pub fn negotiate_hdr_size(features: u64) -> usize {
    if features & (1u64 << 15) != 0 { 12 } else { 10 } // bit15 = VIRTIO_NET_F_MRG_RXBUF
}
```

---

## 7. Mémoire et performance

### 7.1 Empreinte complète `.bss` / DMA

| Zone | Taille | Allocation |
|---|---|---|
| `TCP_RX_BUFS` (64 × 4096) | 256 Ko | `.bss` static |
| `TCP_TX_BUFS` (64 × 4096) | 256 Ko | `.bss` static |
| `TCP_STATE_STORE` (64 × 6176) | ~386 Ko | `.bss` static |
| `SOCKET_STORAGE` | ~1 Ko | `.bss` static |
| **Total `.bss`** | **~899 Ko** | — |
| NetBufPool RX (256 × PAGE_SIZE) | 1 Mo | DMA (hors ELF) |
| NetBufPool TX (256 × PAGE_SIZE) | 1 Mo | DMA (hors ELF) |
| **Total réseau** | **~2.9 Mo** | 0 alloc dynamique |

### 7.2 Chemin chaud RX (latence estimée)

```
virtio_net pop_used + push SpscRing IPC   ~300 ns
network_server SpscRing pop_into()        ~ 50 ns
ExoNetDevice::receive()                   ~ 30 ns
smoltcp poll_ingress_single()             ~600 ns
ExoRxToken::consume() + released_buf     ~ 20 ns
flush_released() → IPC send              ~200 ns
virtio_net process_rx_releases + refill  ~100 ns
────────────────────────────────────────────────
Total (1 paquet, cas nominal)            ~1.3 µs
```

### 7.3 Cibles Phase 1

| Métrique | Cible | Méthode |
|---|---|---|
| Throughput TCP RX | ≥ 3 Gbps | iperf3 QEMU |
| Throughput TCP TX | ≥ 6 Gbps | iperf3 QEMU |
| Latence RTT loopback | ≤ 100 µs | ping QEMU |
| Allocations heap chemin chaud | 0 | audit statique |
| Sérialisation TcpStateStore | ≤ 1 ms | TSC delta |

---

## 8. Intégration ExoOS

### 8.1 Règles Ring 1

| Règle | Application V4 |
|---|---|
| `PHX-01` | `isolation.rs` : `drain_all()` + `serialize_into()` + `PrepareIsolationAck` |
| `PHX-02` | `no_std` + `panic=abort` + `poll_ingress_single()` borné |
| `PHX-03` | Blake3(ELF) enregistré ExoFS au boot |
| `SRV-01` | `ipc_broker::register("network_server")` après `CAP-01` |
| `SRV-02` | Crypto → crypto_server (PID 4) via IPC (TLS Phase 3) |
| `SRV-04` | `ZERO_BLOB_ID_4K` non utilisé |
| `CAP-01` | `verify_cap_token()` première instruction de `_start()` |
| `IPC-01` | `SpscRing` utilisé via son API existante (`push_copy`/`pop_into`) |
| `IPC-02` | `NetMsg` (48B), `NetReply` (48B), `DriverInitMsg` (32B), `RxReleaseMsg` (48B) — Sized, fixe |
| `IPC-03` | network_server n'appelle jamais vfs_server ni lui-même |

### 8.2 Séquence de boot Ring 1

```
Étape  1  : ipc_broker  (PID 2)
Étape  2  : memory_server
Étape  3  : init_server (PID 1)
Étape  4  : vfs_server  (PID 3)
Étape  5  : crypto_server (PID 4)
Étape  6  : device_server
Étape  7  : virtio-block
Étape  8  : virtio-sound / virtio-console (optionnels)
Étape  9  : virtio-net  ← démarre, attend DriverInitMsg
Étape 10  : network_server ← alloue DMA, envoie DriverInitMsg, reçoit MAC_REPLY
Étape 11  : scheduler_server
Étape 12  : exo_shield (Phase 3)
```

---

## 9. Hors scope (toutes phases)

| Feature | Raison |
|---|---|
| DPDK | Linux userspace — impossible no_std bare-metal |
| XDP/eBPF | Nécessite JIT BPF — hors scope |
| QUIC natif | Architecturalement userspace |
| `SpscRing<u16>` ou `U16Ring` en SHM | SHM inter-process non câblé dans ExoOS |
| `DriverInitMsg._virt` | Adresses virtuelles non transférables inter-process |
| `dpdk_bridge.rs`, `xdp.rs`, `io_uring_sock.rs` | Supprimés |
| UDP Phoenix (TcpStateStore) | Phase 2 — sockets UDP perdues après cycle Phoenix Phase 1 |
| ExoSocket natif (SYS 547–550) | Phase 2 — collision `SYSCALL_TABLE_SIZE=547` |

---

## 10. Ordre d'implémentation

```
Phase 1a — network_server (zero kernel touch)
  1. protocol.rs      : NetMsg, NetReply, DriverInitMsg, RxReleaseMsg + assert! size
  2. buf_pool.rs      : NetBufPool via SYS_DMA_ALLOC, accès (virt, iova) séparés
  3. virtio_device.rs : ExoRxToken + released_buf, ExoTxToken, ExoNetDevice
  4. smoltcp_iface.rs : static bufs, SmoltcpIface::init(device), poll_one/egress
  5. socket_table.rs  : SocketHandle ∈ smoltcp::iface
  6. tcp_store.rs     : TcpStateStore const new_empty(), assert! size
  7. isolation.rs     : drain_all + serialize_into + ACK
  8. driver_link.rs   : connect_virtio_net (DriverInitMsg + MAC_QUERY) + flush_released

Phase 1b — virtio_net driver
  9. net.rs           : populate_rx_descriptors, handle_rx_irq (sans touch rx_submitted),
                        process_rx_releases (refill sur RxReleaseMsg), flush_tx
                        POLL_THRESHOLD=32, negotiate_hdr_size
 10. main.rs          : boucle [process_rx_releases → handle_rx_irq → flush_tx]

Phase 1c — kernel (consensus IA requis)
 11. net_bridge.rs    : 15 fonctions, lazy lookup, bridge_result
 12. MOD-01 à MOD-04
 13. Tests : socket/connect/send/recv/getpeername depuis Ring 3
```

---

## Annexe — Index complet des sources de corrections

| ID | Source | Intégré depuis |
|---|---|---|
| CORR-B-01 | beta | V3.0 : NetMsg 48B |
| CORR-B-02 | beta | V3.0 : smoltcp Device trait GAT |
| CORR-B-03 | beta | V3.0 : getpeername (15e syscall) |
| CORR-B-04 | beta | V3.0 : lazy lookup race boot |
| CORR-B-05 | beta | V3.0 : pas de .expect() |
| BOGUE-02 | gamma | V3.0 : SocketHandle ∈ smoltcp::iface |
| MANQUE-01 | gamma | V3.0 : refill_rx_descriptors après consume |
| MANQUE-02 | gamma | V3.0 : virtio_net_hdr 10 bytes |
| NOTE-01 | gamma | V3.0 : smoltcp 0.12 |
| BUG-V3-01 | gamma | V3.1 : SocketSet<'static> statics |
| BUG-V3-02 | gamma | V3.1 : receive() découplé TX |
| BUG-V3-03 | gamma | V3.1 : TcpStateStore static |
| BUG-V3-04 | gamma | V3.1 : Interface::new() sans dummy |
| RACE-RX | beta | V3.2 → V4 : released_ring IPC |
| COMMENT-RX | beta | V3.2 : commentaire exact |
| POINT-2 | gamma V3.2 | V4 : released_ring_ptr = virt → supprimé |
| POINT-3 | gamma V3.2 | V4 : unwrap_unchecked() hoisté |
| **V4-ALPHA-01** | alpha | **V4 : SpscRing non-générique — pas de SpscRing\<u16\>** |
| **V4-ALPHA-02** | alpha | **V4 : SHM inter-process non câblé → IPC pur** |
| **V4-ALPHA-03** | alpha | **V4 : IOVA vs virt — SYS_DMA_ALLOC retourne (virt,iova)** |
| **V4-ALPHA-04** | alpha | **V4 : RxReleaseMsg 48B = 4 + 4 + 20×2 (calcul exact)** |
| **V4-ALPHA-05** | alpha | **V4 : rx_submitted dans virtio_net, released_buf dans ns** |

---

*ExoOS · Network Module v4.0 · Mai 2026*  
*Claude Alpha — Synthèse complète V1→V3.2, réécriture de zéro*  
*Fondée sur lecture directe repo + smoltcp 0.12 docs.rs + virtio-spec 1.1*
