# Endpoints IPC

Les endpoints sont les **points de communication nommés** du système IPC. Ils jouent le rôle d'adresses de destination stables : un serveur crée un endpoint, y attache un nom, et des clients s'y connectent pour obtenir des canaux bidirectionnels.

## Vue d'ensemble

```
endpoint/
├── descriptor.rs  — EndpointDesc : structure centrale d'un endpoint
├── registry.rs    — Registre nom → EndpointId (Robin Hood hash)
├── connection.rs  — Handshake connect / accept
└── lifecycle.rs   — Création, destruction, cleanup RAII
```

---

## Descripteur d'endpoint — `endpoint/descriptor.rs`

### Structure

```rust
pub struct EndpointDesc {
    id:        EndpointId,
    owner_pid: ProcessId,
    state:     EndpointState,
    backlog:   [PendingConnection; MAX_BACKLOG],
    n_pending: usize,
    cap_token: CapToken,
}

pub enum EndpointState {
    Idle,
    Listening,
    Closed,
}

pub struct PendingConnection {
    client_pid: ProcessId,
    cookie:     Cookie,
    timestamp:  u64,
}
```

### Accès sûr aux champs

Le fichier utilise `addr_of!()` pour les mutations en place afin d'éviter les violations UB liées aux champs `repr(C)` ou aux accès partiels :

```rust
// Pattern recommandé pour les mutations d'EndpointDesc
let ptr = addr_of_mut!(desc.n_pending);
unsafe { ptr.write(ptr.read() + 1); }
```

---

## Registre des endpoints — `endpoint/registry.rs`

### Description

Table de hachage Robin Hood mappant un nom (chaîne statique ou hash 64 bits) vers un `EndpointId`. Résolution en O(1) amortie.

### Constantes

```rust
const REGISTRY_CAPACITY: usize = IPC_MAX_ENDPOINTS;  // 1 024 slots
```

### API

```rust
pub fn registry_lookup(name: u64) -> Option<EndpointId>
pub fn registry_insert(name: u64, id: EndpointId) -> Result<(), IpcError>
pub fn registry_remove(name: u64) -> Option<EndpointId>
```

### Algorithme Robin Hood

L'algorithme Robin Hood réduit la variance des longueurs de chaînes de collision. Lors d'une insertion :
- Si la sonde actuelle est plus proche de sa position idéale que l'élément en place, les deux sont échangés.
- L'élément déplacé continue sa sonde, garantissant une profondeur maximale logarithmique.

### Nommage

Les endpoints sont identifiés par un hash 64 bits de leur nom lisible. Le calcul du hash est à la charge de l'appelant. Exemple :

```rust
fn name_hash(s: &[u8]) -> u64 {
    // FNV-1a 64 bits
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
```

---

## Connexion et handshake — `endpoint/connection.rs`

### Flux de connexion

```
Client                          Serveur
────────────────────────────────────────────────────────
endpoint_connect(ep_id, pid)
  1. Vérifie capability (IPC-04)
  2. Génère Cookie aléatoire
  3. PendingConnection → backlog du serveur
  4. futex_wait (attend accept)

                                endpoint_accept(ep_id)
                                  5. Pop PendingConnection
                                  6. Alloue ChannelId
                                  7. Crée SpscRing×2 (A→B, B→A)
                                  8. futex_wake → débloque client

  9. Client reçoit ChannelId
  → canal bidirectionnel établi
```

### Handshake

Le handshake vérifie :
1. **Capability** : le client détient un `CapToken` avec droit `Rights::CONNECT` sur l'endpoint
2. **Version de protocole** : `IPC_VERSION` doit correspondre
3. **Magic** : `MSG_HEADER_MAGIC` présent dans le premier message

En cas d'échec : `Err(IpcError::HandshakeFailed)`.

### API

```rust
pub fn do_connect(ep: EndpointId, client: ProcessId) -> Result<ChannelId, IpcError>
pub fn do_accept(ep: EndpointId) -> Result<(ChannelId, ProcessId), IpcError>
```

---

## Cycle de vie — `endpoint/lifecycle.rs`

### Pool d'endpoints

```rust
static ENDPOINT_POOL: SpinLock<[Option<EndpointDesc>; MAX_ENDPOINTS]> =
    SpinLock::new([const { None }; MAX_ENDPOINTS]);
```

`MAX_ENDPOINTS` = `IPC_MAX_ENDPOINTS` = 1 024.

### Création

```rust
pub fn endpoint_create(owner: ProcessId) -> Result<EndpointId, IpcError>
```

1. Prend le verrou sur `ENDPOINT_POOL`
2. Trouve le premier slot `None`
3. Alloue un `EndpointId` monotone via `alloc_endpoint_id()`
4. Initialise `EndpointDesc { state: Idle, n_pending: 0, ... }`
5. Retourne l'`EndpointId`

### Destruction

```rust
pub fn endpoint_destroy(id: EndpointId) -> Result<(), IpcError>
```

1. Passe l'endpoint en état `Closed`
2. Réveille tous les threads en attente (clients bloqués sur `endpoint_connect`)
3. Libère les connexions pendantes dans le backlog
4. Met le slot à `None` dans le pool

### Fonctions d'écoute

```rust
pub fn endpoint_listen(id: EndpointId) -> Result<(), IpcError>
// Passe l'endpoint de Idle → Listening

pub fn endpoint_close(id: EndpointId) -> Result<(), IpcError>
// Equivalent de destroy (état Closed) sans libérer définitivement
```

---

## Fonctions publiques (depuis `ipc/mod.rs`)

```rust
pub fn endpoint_create(owner: ProcessId) -> Result<EndpointId, IpcError>
pub fn endpoint_destroy(id: EndpointId)  -> Result<(), IpcError>
pub fn endpoint_listen(id: EndpointId)   -> Result<(), IpcError>
pub fn endpoint_close(id: EndpointId)    -> Result<(), IpcError>
pub fn endpoint_connect(id: EndpointId, from: ProcessId) -> Result<ChannelId, IpcError>
pub fn endpoint_accept(id: EndpointId)   -> Result<(ChannelId, ProcessId), IpcError>
```

---

## Modèle de sécurité

Les endpoints sont protégés par le système de capabilities (`IPC-04`) :

| Opération | Droit requis |
|---|---|
| `endpoint_connect` | `Rights::CONNECT` |
| `endpoint_accept` | `Rights::LISTEN` |
| `sync_channel_send` | `Rights::SEND` |
| `sync_channel_recv` | `Rights::RECEIVE` |

La vérification est effectuée par `capability_bridge::verify_endpoint_access()`, qui délègue à `security::capability::verify()`. Aucune logique de droit n'existe dans `endpoint/`.
