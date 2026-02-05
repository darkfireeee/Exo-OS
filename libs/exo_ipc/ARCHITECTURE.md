# Architecture exo_ipc v0.2.0 - Vue d'ensemble

## 📁 Structure des Fichiers (25 fichiers)

```
exo_ipc/
│
├── 📄 Cargo.toml                      # Configuration du package
├── 📄 README.md                       # Documentation principale
├── 📄 CHANGELOG.md                    # Historique des changements
├── 📄 REFONTE_COMPLETE.md            # Rapport de refonte
│
└── src/
    ├── 📄 lib.rs                      # Point d'entrée, réexportations
    │
    ├── types/                         # Types fondamentaux (5 fichiers)
    │   ├── 📄 mod.rs                  # Module types
    │   ├── 📄 message.rs              # Messages IPC (128B alignés)
    │   ├── 📄 endpoint.rs             # Endpoints et adressage
    │   ├── 📄 capability.rs           # Sécurité capability-based
    │   └── 📄 error.rs                # Gestion d'erreurs (15+ types)
    │
    ├── ring/                          # Ring buffers lock-free (3 fichiers)
    │   ├── 📄 mod.rs                  # Module ring
    │   ├── 📄 spsc.rs                 # Single Producer Single Consumer
    │   └── 📄 mpsc.rs                 # Multi Producer Single Consumer
    │
    ├── channel/                       # APIs de canaux (2 fichiers)
    │   ├── 📄 mod.rs                  # Module channel
    │   └── 📄 bounded.rs              # Canaux avec capacité fixe
    │
    ├── shm/                           # Mémoire partagée (3 fichiers)
    │   ├── 📄 mod.rs                  # Module shm
    │   ├── 📄 region.rs               # Régions mémoire zero-copy
    │   └── 📄 pool.rs                 # Pool de messages
    │
    ├── protocol/                      # Protocoles IPC (3 fichiers)
    │   ├── 📄 mod.rs                  # Module protocol
    │   ├── 📄 handshake.rs            # Négociation de version
    │   └── 📄 flow_control.rs         # Flow control algorithms
    │
    └── util/                          # Utilitaires (4 fichiers)
        ├── 📄 mod.rs                  # Module util
        ├── 📄 atomic.rs               # Helpers atomiques
        ├── 📄 cache.rs                # Cache-line padding
        └── 📄 checksum.rs             # CRC32C et checksums

# Total: 25 fichiers, ~2500 lignes de code
```

## 🔗 Dépendances entre Modules

```
lib.rs
  ├─→ types/       (fondamental, utilisé par tous)
  │    ├─→ message
  │    ├─→ endpoint
  │    ├─→ capability
  │    └─→ error
  │
  ├─→ util/        (utilisé par ring/, channel/, shm/)
  │    ├─→ atomic
  │    ├─→ cache
  │    └─→ checksum
  │
  ├─→ ring/        (utilisé par channel/)
  │    ├─→ spsc    (dépend: types, util)
  │    └─→ mpsc    (dépend: types, util)
  │
  ├─→ channel/     (API haut niveau)
  │    └─→ bounded (dépend: ring, types, util)
  │
  ├─→ shm/         (mémoire partagée)
  │    ├─→ region  (dépend: types, util)
  │    └─→ pool    (dépend: types)
  │
  └─→ protocol/    (handshake, flow control)
       ├─→ handshake     (dépend: types)
       └─→ flow_control  (dépend: util)
```

## 📦 Exports Publics

### Depuis `lib.rs`

```rust
// Types fondamentaux
pub use types::{
    Message, MessageType, MessageFlags, MessageHeader,
    IpcError, IpcResult, RecvError, SendError,
    Endpoint, EndpointId, EndpointType,
    Capability, CapabilityId, Permissions,
    MAX_INLINE_SIZE, MESSAGE_SIZE, PROTOCOL_VERSION,
};

// Canaux
pub use channel::{
    spsc, mpsc,                    // Fonctions de création
    SenderSpsc, ReceiverSpsc,      // Types SPSC
    SenderMpsc, ReceiverMpsc,      // Types MPSC
};

// Mémoire partagée
pub use shm::{
    SharedRegion, SharedMapping,   // Régions
    MessagePool,                   // Pool
    RegionId, RegionPermissions,
};

// Protocoles
pub use protocol::{
    HandshakeManager,              // Handshake
    SessionConfig, Capabilities,
    TokenBucketFlowController,     // Flow control
    CreditBasedFlowController,
};

// Utilitaires
pub use util::{
    AtomicStats, SequenceCounter,  // Atomics
    CachePadded, CACHE_LINE_SIZE,  // Cache
    crc32c,                        // Checksums
};
```

## 📊 Statistiques par Module

| Module | Fichiers | LOC (approx) | Tests | Complexité |
|--------|----------|--------------|-------|------------|
| types/ | 5 | 600 | ✅ | Faible |
| ring/ | 3 | 400 | ✅ | Élevée (lock-free) |
| channel/ | 2 | 500 | ✅ | Moyenne |
| shm/ | 3 | 350 | ✅ | Moyenne |
| protocol/ | 3 | 450 | ✅ | Moyenne |
| util/ | 4 | 450 | ✅ | Moyenne |
| **Total** | **20** | **~2750** | **✅** | - |

## 🎯 Points d'Entrée (API Publique)

### 1. Créer un Canal SPSC
```rust
let (tx, rx) = exo_ipc::channel::spsc(64)?;
```

### 2. Créer un Canal MPSC
```rust
let (tx, rx) = exo_ipc::channel::mpsc(128)?;
```

### 3. Créer une Région Partagée
```rust
let region = exo_ipc::shm::SharedRegion::new(4096, permissions)?;
```

### 4. Créer un Pool de Messages
```rust
let pool = exo_ipc::shm::MessagePool::new(16, 256);
```

### 5. Handshake
```rust
let handshake = exo_ipc::protocol::HandshakeManager::new(config);
```

### 6. Flow Control
```rust
let flow = exo_ipc::protocol::TokenBucketFlowController::new(100, 50);
```

## 🔧 Structures de Données Clés

### Message (128 bytes)
```rust
#[repr(C, align(64))]
pub struct Message {
    header: MessageHeader,    // 64B
    payload: MessagePayload,  // 64B (union)
}
```

### Ring Buffer SPSC
```rust
pub struct SpscRing {
    buffer: *mut Message,           // Heap
    capacity: usize,                // Puissance de 2
    mask: usize,                    // Pour wrapping
    head: CachePadded<RingIndex>,   // Séparé cache-line
    tail: CachePadded<RingIndex>,   // Séparé cache-line
}
```

### Shared Region
```rust
pub struct SharedRegion {
    ptr: NonNull<u8>,               // Mémoire partagée
    metadata: Arc<RegionMetadata>,  // Ref-counted
}
```

## 🚀 Chemins Critiques (Performance)

### Envoi de Message (Fast Path)
```
tx.send(msg)
  └─→ ring.push(msg)
       └─→ head.load(Relaxed)           // Pas de sync
       └─→ tail.load(Acquire)           // Sync read
       └─→ Vérifier espace disponible
       └─→ ptr::write(slot, msg)        // Unsafe copy
       └─→ head.store(Release)          // Sync write
```

### Réception de Message (Fast Path)
```
rx.recv()
  └─→ ring.pop()
       └─→ tail.load(Relaxed)           // Pas de sync
       └─→ head.load(Acquire)           // Sync read
       └─→ Vérifier si vide
       └─→ ptr::read(slot)              // Unsafe read
       └─→ tail.store(Release)          // Sync write
```

## 🛡️ Garanties de Sécurité

1. **Memory Safety**: Tous les unsafe sont encapsulés et justifiés
2. **Thread Safety**: Send/Sync implémentés correctement
3. **No Data Races**: Atomics avec orderings appropriés
4. **No Use-After-Free**: Drop implémenté, ref-counting
5. **No Undefined Behavior**: Validation complète des indices

## 📚 Documentation

- **README.md**: Guide utilisateur complet (400+ lignes)
- **CHANGELOG.md**: Historique détaillé avec migration guide
- **REFONTE_COMPLETE.md**: Rapport technique approfondi
- **Inline docs**: Tous items publics documentés
- **Exemples**: 6+ cas d'usage dans README

---

**Architecture conçue pour performance, robustesse et maintenabilité** 🏗️
