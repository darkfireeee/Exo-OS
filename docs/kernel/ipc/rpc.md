# RPC IPC

Le sous-module `ipc/rpc/` implémente un mécanisme d'appel de procédure à distance (Remote Procedure Call) binaire, léger, sans allocation dynamique.

## Vue d'ensemble

```
rpc/
├── protocol.rs  — MethodId, RpcStatus, RPC_MAGIC, en-tête binaire
├── server.rs    — RpcServer — registration et dispatch de méthodes
├── client.rs    — RpcClient — stub + boucle de retry
└── timeout.rs   — RpcTimeout — fn pointer injectée (pas de time source fixe)
```

---

## Protocole RPC — `rpc/protocol.rs`

### Constantes

```rust
pub const RPC_MAGIC:   u32 = 0xE5_0C_A1_1C;  // "EXO CALL"
pub const RPC_VERSION: u16 = 1;
```

### Types fondamentaux

```rust
/// Identifiant opaque d'une méthode RPC (défini par le serveur).
pub struct MethodId(pub u32);

/// Code de statut retourné par un appel RPC.
pub enum RpcStatus {
    Ok           = 0,
    MethodNotFound = 1,
    InvalidArgs  = 2,
    Timeout      = 3,
    ServerError  = 4,
}
```

### Cadre RPC

```rust
#[repr(C)]
pub struct RpcFrameHeader {
    magic:     u32,     // RPC_MAGIC
    version:   u16,     // RPC_VERSION
    status:    u8,      // RpcStatus (réponse) ou 0 (requête)
    _pad:      u8,
    method_id: u32,     // MethodId
    call_id:   u64,     // corrélation requête/réponse (Cookie)
    data_len:  u32,
}
```

### Validation

Toute trame reçue est validée :
1. `magic == RPC_MAGIC` → sinon `Err(IpcError::ProtocolError)`
2. `version == RPC_VERSION` → sinon `Err(IpcError::HandshakeFailed)`
3. `data_len <= MAX_MSG_SIZE - size_of::<RpcFrameHeader>()` → sinon `Err(IpcError::MessageTooLarge)`

---

## Serveur RPC — `rpc/server.rs`

### Description

Dispatcher de méthodes RPC. Chaque méthode est une fonction `fn(&IpcMessage) -> IpcMessage` enregistrée avec un `MethodId`.

### Structure

```rust
const MAX_RPC_METHODS: usize = 64;

pub struct RpcServer {
    endpoint: AtomicU64,   // EndpointId sous-jacent (AtomicU64, pas u32)
    methods:  [(MethodId, fn(&IpcMessage) -> IpcMessage); MAX_RPC_METHODS],
    n_methods: usize,
}
```

**Note** : `endpoint` est stocké comme `AtomicU64` pour permettre la mise à jour atomique sans verrou sur les architectures 64 bits.

### API

```rust
pub fn rpc_server_create(ep: EndpointId) -> Result<RpcServer, IpcError>

pub fn rpc_server_register(
    server:  &mut RpcServer,
    method:  MethodId,
    handler: fn(&IpcMessage) -> IpcMessage,
) -> Result<(), IpcError>

pub fn rpc_server_dispatch(server: &RpcServer) -> Result<(), IpcError>
```

### Algorithme `rpc_server_dispatch()`

```
loop:
  msg = sync_channel_recv(server.endpoint)?
  header = parse_rpc_header(msg)?
  handler = find_handler(header.method_id)?
  response = handler(&msg)
  sync_channel_send(requester_ep, response)?
```

Si `method_id` est inconnu : la réponse contient `RpcStatus::MethodNotFound`.

---

## Client RPC — `rpc/client.rs`

### Description

Stub client qui envoie une requête RPC et attend la réponse, avec retry automatique en cas de timeout.

### Structure

```rust
pub struct RpcClient {
    client_ep: AtomicU64,   // EndpointId du canal retour (AtomicU64)
    server_ep: EndpointId,
    timeout:   RpcTimeout,
    next_id:   AtomicU64,   // générateur de call_id
}
```

### API

```rust
pub fn rpc_client_create(server_ep: EndpointId) -> Result<RpcClient, IpcError>

pub fn rpc_call(
    client:   &RpcClient,
    method:   MethodId,
    request:  &IpcMessage,
    response: &mut IpcMessage,
) -> Result<(), IpcError>
```

### Algorithme `rpc_call()`

```
call_id = client.next_id.fetch_add(1, Relaxed)
build request frame (RPC_MAGIC, method, call_id, ...)

retries = 0
loop:
  sync_channel_send(server_ep, req)?
  match sync_channel_recv_timeout(client_ep, timeout):
    Ok(resp) if resp.call_id == call_id → break
    Ok(_)    → ignorer (réponse à un ancien appel)
    Err(Timeout) if retries < MAX_RETRIES →
        retries += 1; continue
    Err(e) → return Err(e)

response = resp
```

### Création des EndpointId dans le client

```rust
// Widening sûr pour les IDs 64 bits :
let ep = unsafe { NonZeroU64::new_unchecked(raw_id) };
let endpoint_id = EndpointId(ep);
```

---

## Timeout RPC — `rpc/timeout.rs`

### Description

Injecte une source de temps dans le client RPC sans dépendre d'un sous-système temps global. Utilise un fn pointer pour rester `no_std` compatible.

### Structure

```rust
pub struct RpcTimeout {
    pub timeout_ns: u64,             // durée max d'attente
    pub time_fn:    fn() -> u64,     // retourne le temps courant en ns
}

impl RpcTimeout {
    pub fn new(timeout_ns: u64, time_fn: fn() -> u64) -> Self
    pub fn has_expired(&self, start_ns: u64) -> bool {
        (self.time_fn)() - start_ns >= self.timeout_ns
    }
}
```

### Injection de la source temps

```rust
pub fn install_time_fn(f: fn() -> u64)
```

Doit être appelée pendant l'initialisation du kernel, après la configuration de l'horloge (HPET, TSC). Exemple :

```rust
// Dans kernel_main, après timer_init() :
ipc::rpc::timeout::install_time_fn(|| rdtsc_ns());
```

Si `install_time_fn` n'est pas appelée, `time_fn` retourne `0` (mode dégradé : pas de timeout).

---

## Exemple complet d'usage

### Côté serveur

```rust
// Boot
let server_ep = endpoint_create(my_pid)?;
endpoint_listen(server_ep)?;

let mut rpc_srv = rpc_server_create(server_ep)?;
rpc_server_register(&mut rpc_srv, MethodId(1), handle_ping)?;
rpc_server_register(&mut rpc_srv, MethodId(2), handle_status)?;

// Boucle principale
loop {
    rpc_server_dispatch(&rpc_srv)?;
}

fn handle_ping(req: &IpcMessage) -> IpcMessage {
    msg_data(req.dst, req.src, b"pong")
}
```

### Côté client

```rust
let client = rpc_client_create(server_ep)?;
let req = msg_data(my_ep, server_ep, b"ping");
let mut resp = IpcMessage::default();
rpc_call(&client, MethodId(1), &req, &mut resp)?;
// resp.data contient "pong"
```
