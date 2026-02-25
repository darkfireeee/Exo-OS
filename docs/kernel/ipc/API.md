# Surface publique IPC — Référence API

Ce document liste tous les symboles exportés par `kernel/src/ipc/mod.rs` et utilisables par les modules consommateurs du noyau.

> **Règle** : Les couches supérieures n'importent que depuis `crate::ipc::*`. Aucun import direct depuis les sous-modules internes.

---

## Types fondamentaux (`core/types.rs`)

### Identifiants opaques

```rust
pub struct MessageId(pub NonZeroU64);
pub struct ChannelId(pub NonZeroU64);
pub struct EndpointId(pub NonZeroU64);
pub struct Cookie(pub u64);
pub use crate::scheduler::ProcessId;  // re-export
```

| Type | Description | Sentinelle |
|---|---|---|
| `MessageId` | ID unique monotone d'un message | — |
| `ChannelId` | ID d'un canal | `ChannelId::DANGLING` = `u64::MAX` |
| `EndpointId` | ID d'un endpoint nommé | `EndpointId::INVALID` = `u64::MAX` |
| `Cookie` | Valeur 64 bits opaque (corrélation req/réponse) | `Cookie::ZERO` |
| `ProcessId` | ID de processus (depuis `scheduler`) | — |

### Générateurs monotones

```rust
pub fn alloc_message_id()  -> MessageId
pub fn alloc_channel_id()  -> ChannelId
pub fn alloc_endpoint_id() -> EndpointId
```

Generateurs thread-safe (`Ordering::Relaxed`). Panique en debug si rollover.

### Drapeaux de message

```rust
pub struct MsgFlags(pub u32);
impl MsgFlags {
    pub const RT:        Self;  // temps-réel
    pub const REPLY:     Self;  // réponse
    pub const ZEROCOPY:  Self;  // référence physique
    pub const BROADCAST: Self;  // livrer à tous
    pub const ERROR:     Self;  // message d'erreur
    pub const SYNC:      Self;  // attend acquittement
    pub const NOWAIT:    Self;  // non bloquant
}

pub struct MessageFlags(pub u16);  // version 16 bits (MsgFrameHeader)
impl MessageFlags { /* mêmes constantes */ }
```

### Types de message

```rust
pub enum MessageType {
    Data     = 0,
    Control  = 1,
    Signal   = 2,
    RpcReply = 3,
}
```

### Erreurs

```rust
pub enum IpcError {
    WouldBlock         = 1,
    EndpointNotFound   = 2,
    ChannelClosed      = 3,
    PermissionDenied   = 4,
    MessageTooLarge    = 5,
    Timeout            = 6,
    ResourceExhausted  = 7,
    ConnRefused        = 8,
    AlreadyConnected   = 9,
    InvalidParam       = 10,
    HandshakeFailed    = 11,
    Interrupted        = 12,
    InternalError      = 13,
    ShmPoolFull        = 14,
    OutOfOrder         = 15,
    InvalidHandle      = 16,
    Closed             = 17,
    Internal           = 18,
    Invalid            = 19,
    Full               = 20,
    Loop               = 21,
    NotFound           = 22,
    NullEndpoint       = 23,
    InvalidEndpoint    = 24,
    Retry              = 25,
    InvalidArgument    = 26,
    OutOfResources     = 27,
    QueueFull          = 28,
    QueueEmpty         = 29,
    ProtocolError      = 30,
    MappingFailed      = 31,
}
```

```rust
pub enum IpcCapError {
    Revoked            = 1,
    ObjectNotFound     = 2,
    InsufficientRights = 3,
    DelegationDenied   = 4,
}
impl From<IpcCapError> for IpcError { ... }
```

---

## Constantes (`core/constants.rs`)

```rust
pub const MAX_MSG_SIZE:            usize = 240;
pub const RING_SIZE:               usize = 16;
pub const RING_MASK:               usize = 15;
pub const IPC_VERSION:             u32   = 1;
pub const IPC_MAX_CHANNELS:        usize = 4_096;
pub const IPC_MAX_ENDPOINTS:       usize = 1_024;
pub const IPC_MAX_PROCESSES:       usize = 512;
pub const MSG_HEADER_MAGIC:        u32   = 0x1FCF_07E0;
pub const SYNC_CHANNEL_TIMEOUT_NS: u64   = 100_000_000;
pub const FUSION_RING_SIZE:        usize = /* voir source */;
pub const FUSION_BATCH_THRESHOLD:  usize = 4;
```

---

## Mitigation Spectre v1

```rust
/// Retourne un index sûr pour accès dans un tableau de taille `size`.
/// Si index >= size → retourne 0 (pas d'accès spéculatif hors-borne).
#[inline(always)]
pub fn array_index_nospec(index: usize, size: usize) -> usize;
```

Source : `kernel/src/ipc/core/types.rs`

---

## Statistiques globales

```rust
pub static IPC_STATS: IpcStatsCounter;

pub struct IpcStatsSnapshot {
    pub msgs_sent:     u64,
    pub msgs_received: u64,
    pub msgs_dropped:  u64,
    pub shm_allocs:    u64,
    pub shm_frees:     u64,
    pub ep_creates:    u64,
    pub ep_destroys:   u64,
    pub cap_checks:    u64,
    pub cap_denials:   u64,
}

impl IpcStatsCounter {
    pub fn snapshot(&self) -> IpcStatsSnapshot;
    pub fn reset_all(&self);
}
```

Source : `kernel/src/ipc/stats/counters.rs`

---

## Endpoints

```rust
pub fn endpoint_create(owner: ProcessId) -> Result<EndpointId, IpcError>
pub fn endpoint_destroy(id: EndpointId) -> Result<(), IpcError>
pub fn endpoint_listen(id: EndpointId) -> Result<(), IpcError>
pub fn endpoint_close(id: EndpointId) -> Result<(), IpcError>
pub fn endpoint_connect(id: EndpointId, from: ProcessId) -> Result<ChannelId, IpcError>
pub fn endpoint_accept(id: EndpointId) -> Result<(ChannelId, ProcessId), IpcError>
```

Source : `kernel/src/ipc/endpoint/`

---

## Canaux synchrones

```rust
pub fn sync_channel_send(ch: ChannelId, msg: &IpcMessage) -> Result<(), IpcError>
pub fn sync_channel_recv(ch: ChannelId, buf: &mut IpcMessage) -> Result<(), IpcError>
pub fn sync_channel_close(ch: ChannelId) -> Result<(), IpcError>
```

Source : `kernel/src/ipc/channel/sync.rs`

---

## Mémoire partagée

```rust
pub fn shm_alloc(size: usize) -> Result<*mut u8, IpcError>
pub fn shm_free(ptr: *mut u8) -> Result<(), IpcError>
pub fn shm_map(ptr: *mut u8, pid: ProcessId) -> Result<*mut u8, IpcError>
pub fn shm_unmap(ptr: *mut u8, pid: ProcessId) -> Result<(), IpcError>
```

Source : `kernel/src/ipc/shared_memory/`

---

## Synchronisation — Futex

```rust
pub struct FutexKey(u64);
impl FutexKey {
    pub fn from_addr(addr: &AtomicU32) -> Self;
}

pub enum WaiterState {
    Woken          = 0,
    ValueMismatch  = 1,
    Cancelled      = 2,
}

pub struct FutexIpcStats {
    pub waits_total:       u64,
    pub wakes_total:       u64,
    pub timeouts_total:    u64,
    pub value_mismatches:  u64,
}

/// # Safety : appel depuis un contexte thread valide uniquement
pub unsafe fn futex_wait(
    addr:      &AtomicU32,
    key:       FutexKey,
    expected:  u32,
    thread_id: u32,
    spin_max:  u32,
    wake_fn:   WakeFn,
) -> Result<WaiterState, IpcError>

pub unsafe fn futex_wake(key: FutexKey, n: u32) -> u32
pub unsafe fn futex_wake_all(key: FutexKey) -> u32
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter)
pub unsafe fn futex_requeue(src: FutexKey, dst: FutexKey, max_wake: u32, max_requeue: u32)
pub fn futex_stats() -> FutexIpcStats
```

Source : `kernel/src/ipc/sync/futex.rs`

---

## Synchronisation — Events, Barriers, Wait Queue

```rust
pub struct IpcEvent         { ... }
pub struct IpcBarrier       { ... }
pub struct IpcCountingEvent { ... }

pub struct IpcWaiter        { ... }
pub struct IpcWaitQueue     { ... }
pub enum   WakePolicy       { One, All, UpToN(u32) }
pub enum   WakeReason       { Signaled, Timeout, Closed, Interrupted }
```

Source : `kernel/src/ipc/sync/`

---

## Messages

```rust
pub struct IpcMessage { ... }  // builder pattern

// Constructeurs
pub fn msg_data(src: EndpointId, dst: EndpointId, payload: &[u8]) -> IpcMessage
pub fn msg_control(src: EndpointId, dst: EndpointId, payload: &[u8]) -> IpcMessage
pub fn msg_signal(src: EndpointId, dst: EndpointId, signum: u32) -> IpcMessage
```

Source : `kernel/src/ipc/message/builder.rs`

---

## Routeur

```rust
pub fn router_add(from: EndpointId, to: EndpointId) -> Result<(), IpcError>
pub fn router_remove(from: EndpointId) -> Result<(), IpcError>
pub fn router_dispatch(msg: &mut IpcMessage) -> Result<(), IpcError>
```

Source : `kernel/src/ipc/message/router.rs`

---

## RPC Serveur

```rust
pub struct RpcServer { ... }

pub fn rpc_server_create(ep: EndpointId) -> Result<RpcServer, IpcError>
pub fn rpc_server_register(
    server: &mut RpcServer,
    method: MethodId,
    handler: fn(&IpcMessage) -> IpcMessage,
) -> Result<(), IpcError>
pub fn rpc_server_dispatch(server: &RpcServer) -> Result<(), IpcError>
```

Source : `kernel/src/ipc/rpc/server.rs`

---

## RPC Client

```rust
pub struct RpcClient { ... }

pub fn rpc_client_create(server_ep: EndpointId) -> Result<RpcClient, IpcError>
pub fn rpc_call(
    client: &RpcClient,
    method: MethodId,
    request: &IpcMessage,
    response: &mut IpcMessage,
) -> Result<(), IpcError>
```

Source : `kernel/src/ipc/rpc/client.rs`

---

## Initialisation

```rust
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32)
```

Voir [INIT.md](INIT.md) pour le séquençage complet.
