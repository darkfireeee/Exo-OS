# Système de Messages IPC

Le sous-module `ipc/message/` fournit la construction, la sérialisation, le routage et la priorisation des messages IPC.

## Vue d'ensemble

```
message/
├── builder.rs     — IpcMessage, IpcMessageBuilder (fluent)
├── serializer.rs  — Sérialisation zero-copy (capnproto-like)
├── router.rs      — Routage multi-hop (Robin Hood hash)
└── priority.rs    — File prioritaire RT / normal
```

---

## Builder de messages — `message/builder.rs`

### Structure `IpcMessage`

```rust
pub struct IpcMessage {
    pub header: MsgFrameHeader,   // en-tête cadré (magic + métadonnées)
    pub src:    EndpointId,       // endpoint source
    pub dst:    EndpointId,       // endpoint destination
    pub flags:  MessageFlags,     // RT, REPLY, ZEROCOPY, ...
    pub type_:  MessageType,      // Data, Control, Signal, RpcReply
    pub cookie: Cookie,           // corrélation req/réponse (0 si N/A)
    pub data:   [u8; MAX_MSG_SIZE], // payload inline (max 240 octets)
    pub data_len: usize,
}
```

### En-tête de cadre

```rust
pub struct MsgFrameHeader {
    magic:    u32,    // MSG_HEADER_MAGIC = 0x1FCF_07E0
    version:  u16,    // IPC_VERSION = 1
    msg_type: u8,
    flags:    u8,
    msg_id:   u64,    // MessageId monotone
    src_id:   u32,    // partie basse de EndpointId.src
    dst_id:   u32,
    data_len: u32,
}
```

Le champ `magic` est vérifié lors de la désérialisation. Un magic incorrect → `Err(IpcError::ProtocolError)`.

### Fonctions de construction rapide

```rust
/// Message de données brutes.
pub fn msg_data(
    src:     EndpointId,
    dst:     EndpointId,
    payload: &[u8],
) -> IpcMessage

/// Message de contrôle de canal.
pub fn msg_control(
    src:     EndpointId,
    dst:     EndpointId,
    payload: &[u8],
) -> IpcMessage

/// Notification de signal (ex. : Ctrl+C, SIGTERM).
pub fn msg_signal(
    src:    EndpointId,
    dst:    EndpointId,
    signum: u32,
) -> IpcMessage
```

### Builder fluent

```rust
pub struct IpcMessageBuilder {
    msg: IpcMessage,
}

impl IpcMessageBuilder {
    pub fn new() -> Self
    pub fn src(mut self, ep: EndpointId) -> Self
    pub fn dst(mut self, ep: EndpointId) -> Self
    pub fn flags(mut self, f: MessageFlags) -> Self
    pub fn type_(mut self, t: MessageType) -> Self
    pub fn cookie(mut self, c: Cookie) -> Self
    pub fn data(mut self, payload: &[u8]) -> Self   // truncate si > MAX_MSG_SIZE
    pub fn build(self) -> IpcMessage
}

// Exemple :
let msg = IpcMessageBuilder::new()
    .src(my_ep)
    .dst(server_ep)
    .flags(MessageFlags::RT)
    .data(b"hello")
    .build();
```

### Limite de taille

`MAX_MSG_SIZE = 240 octets`. Au-delà, le payload doit être envoyé via le canal streaming avec `ZeroCopyRef` (SHM). La méthode `data()` du builder tronque silencieusement les payloads trop grands en debug et retourne `Err(IpcError::MessageTooLarge)` via le canal.

### Sentinelles d'EndpointId

- `EndpointId::INVALID` (= `u64::MAX`) : destination non encore définie, utilisée comme valeur initiale dans le builder.
- Un message avec `dst == INVALID` ne peut pas être envoyé → `Err(IpcError::InvalidEndpoint)`.

---

## Sérialiseur — `message/serializer.rs`

### Description

Sérialisation et désérialisation zero-copy, inspirée de Cap'n Proto. Les structures sont lues directement depuis leur position mémoire sans copie intermédiaire.

### Modèle

```rust
pub struct IpcSerializer {
    buf:     *mut u8,
    len:     usize,
    written: usize,
}

impl IpcSerializer {
    pub fn new(buf: *mut u8, len: usize) -> Self
    pub fn write_u32(&mut self, v: u32) -> Result<(), IpcError>
    pub fn write_u64(&mut self, v: u64) -> Result<(), IpcError>
    pub fn write_bytes(&mut self, data: &[u8]) -> Result<(), IpcError>
    pub fn finish(self) -> usize  // retourne le nombre d'octets écrits
}

pub struct IpcDeserializer {
    buf:    *const u8,
    len:    usize,
    cursor: usize,
}

impl IpcDeserializer {
    pub fn new(buf: *const u8, len: usize) -> Self
    pub fn read_u32(&mut self) -> Result<u32, IpcError>
    pub fn read_u64(&mut self) -> Result<u64, IpcError>
    pub fn read_bytes(&mut self, dst: &mut [u8]) -> Result<(), IpcError>
}
```

### Conversion d'identifiants

Les identifiants IPC (EndpointId, MessageId) sont des `NonZeroU64`. La sérialisation encode sur 4 octets (partie basse) avec troncature documentée :

```rust
// Encodage (serializer.rs) — narrowing explicite
let src_id = msg.src.0.get() as u32;  // 32 bits suffisent pour les IDs pratiques

// Décodage (deserializer.rs) — widening avec wrapping
let ep = NonZeroU64::new(v as u64).map(EndpointId);
```

---

## Routeur multi-hop — `message/router.rs`

### Description

Table de routage multi-hop implémentée comme une table de hachage Robin Hood. Permet le routage de messages à travers plusieurs sauts (chaînes d'endpoints).

### Modèle

```
Endpoint A ──route──► Endpoint B ──route──► Endpoint C
```

Chaque entrée mappe `from: EndpointId → to: EndpointId`. La résolution est récursive jusqu'à une profondeur maximale (protection contre les boucles).

### Constantes

```rust
const ROUTER_CAPACITY:  usize = 256;   // entrées max dans la table
const MAX_HOP_DEPTH:    usize = 8;     // profondeur maximum avant Loop
```

### API

```rust
pub fn router_add(from: EndpointId, to: EndpointId) -> Result<(), IpcError>
pub fn router_remove(from: EndpointId) -> Result<(), IpcError>
pub fn router_dispatch(msg: &mut IpcMessage) -> Result<(), IpcError>
```

### Détection de boucle

`router_dispatch()` maintient un compteur de sauts. Si la profondeur atteint `MAX_HOP_DEPTH`, retourne `Err(IpcError::Loop)`.

```
router_dispatch(msg) :
  hops = 0
  loop:
    dst = table[msg.dst]?
    if dst == msg.dst → endpoint final, envoyer
    msg.dst = dst
    hops += 1
    if hops >= MAX_HOP_DEPTH → Err(Loop)
```

---

## File prioritaire — `message/priority.rs`

### Description

Deux niveaux de priorité : **RT** (temps-réel) et **Normal**. Les messages RT sont toujours traités avant les messages normaux.

### Modèle

```rust
pub struct PriorityQueue {
    rt_ring:     SpscRing,     // file temps-réel
    normal_ring: SpscRing,     // file normale
}

impl PriorityQueue {
    pub fn push(&self, msg: &RingSlot, flags: MessageFlags) -> Result<(), IpcError>
    pub fn pop(&self, dst: &mut RingSlot) -> bool
}
```

### Algorithme `pop()`

```
1. Tenter pop depuis rt_ring
2. Si vide → pop depuis normal_ring
3. Si les deux vides → retourner false
```

Cette politique garantit que les messages RT ne sont jamais retardés par l'accumulation de messages normaux, sans nécessiter de tri ou de timestamp.

### Interaction avec `MsgFlags::RT`

```rust
// Dans push() :
if flags.contains(MessageFlags::RT) {
    self.rt_ring.push_copy(msg)
} else {
    self.normal_ring.push_copy(msg)
}
```
