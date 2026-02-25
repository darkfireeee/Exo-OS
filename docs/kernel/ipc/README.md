# Module IPC — Communication Inter-Processus

## Vue d'ensemble

Le module `ipc/` constitue la **Couche 2a** du noyau Exo-OS. Il fournit l'intégralité des mécanismes de communication entre processus : canaux synchrones et asynchrones, mémoire partagée, primitives de synchronisation, RPC, routage de messages et endpoints nommés.

---

## Position dans l'architecture en couches

```
┌─────────────────────────────────────────────────────┐
│  Couche 3+  : fs/, process/, userland               │
├─────────────────────────────────────────────────────┤
│  Couche 2a  : ipc/          ← CE MODULE             │
├─────────────────────────────────────────────────────┤
│  Couche 1   : scheduler/                            │
├─────────────────────────────────────────────────────┤
│  Couche 0   : memory/                               │
└─────────────────────────────────────────────────────┘
```

### Dépendances autorisées

| Dépendance | Usage |
|---|---|
| `memory::utils::futex_table` | Table futex globale — `ipc/sync/futex.rs` est un shim pur |
| `memory::phys` / `memory::vmm` | Allocation pages SHM (`NO_COW + PINNED`) |
| `scheduler::ProcessId` | Identifiant de processus réutilisé |
| `scheduler::sync` | Wait/wake thread — `ipc/sync/wait_queue.rs` délègue |
| `security::capability` | Vérification droits — `ipc/capability_bridge/` délègue |

### Dépendances **interdites**

- `fs/` — le module IPC n'importe jamais le VFS
- `process/` — gestion de processus hors scope
- Toute implémentation de futex locale dupliquée

---

## Arborescence des sous-modules

```
ipc/
├── mod.rs                      # Surface publique, ipc_init()
├── core/
│   ├── types.rs                # MessageId, ChannelId, EndpointId, Cookie, IpcError
│   ├── constants.rs            # MAX_MSG_SIZE=240, RING_SIZE=16, IPC_VERSION=1
│   ├── transfer.rs             # MessageHeader, RingSlot, ZeroCopyRef, TransferEngine
│   ├── sequence.rs             # SeqSender, SeqReceiver — garanties d'ordre
│   └── fastcall_asm.s          # Fast IPC (évite l'overhead syscall) — ASM uniquement
├── ring/
│   ├── spsc.rs                 # SPSC ultra-rapide (Release/Acquire)
│   ├── mpmc.rs                 # MPMC lock-free (Dmitry Vyukov)
│   ├── fusion.rs               # Fusion Ring — batching adaptatif EWA
│   ├── slot.rs                 # Gestion de slots (SlotPool, SlotGuard RAII)
│   ├── batch.rs                # Transferts groupés avant flush
│   └── zerocopy.rs             # Partage de pages physiques (ZcRing)
├── channel/
│   ├── sync.rs                 # Canal synchrone (rendezvous, bloquant)
│   ├── async.rs                # Canal asynchrone (futures/waker)
│   ├── mpmc.rs                 # Multi-producteurs multi-consommateurs
│   ├── broadcast.rs            # Un émetteur → N récepteurs (max 16)
│   ├── typed.rs                # Canal type-safe générique
│   └── streaming.rs            # DMA — gros volumes continus
├── shared_memory/
│   ├── page.rs                 # ShmPage — NO_COW + PINNED obligatoires
│   ├── pool.rs                 # Pool pré-alloué (alloc < 100 ns)
│   ├── descriptor.rs           # ShmDescriptor (ID, taille, permissions)
│   ├── mapping.rs              # shm_map/unmap + hooks
│   ├── allocator.rs            # Allocateur SHM lock-free
│   └── numa_aware.rs           # Affinité NUMA locale
├── capability_bridge/
│   ├── mod.rs                  # Re-exports — zéro logique ici
│   └── check.rs                # Shim → security::capability::verify()
├── endpoint/
│   ├── descriptor.rs           # EndpointDesc (ID, owner_pid, backlog)
│   ├── registry.rs             # Registre nom→EndpointId (Robin Hood hash)
│   ├── connection.rs           # Handshake connect/accept
│   └── lifecycle.rs            # Création, destruction, cleanup RAII
├── sync/
│   ├── futex.rs                # Shim IPC → memory::utils::futex_table
│   ├── wait_queue.rs           # IpcWaitQueue — wait/wake avec timeout
│   ├── event.rs                # IpcEvent (notification one-shot)
│   ├── barrier.rs              # IpcBarrier (N participants)
│   └── rendezvous.rs           # Point de rendez-vous symétrique
├── message/
│   ├── builder.rs              # IpcMessageBuilder fluent
│   ├── serializer.rs           # Sérialisation zero-copy (capnproto-like)
│   ├── router.rs               # Routage multi-hop (Robin Hood hash)
│   └── priority.rs             # File prioritaire RT/normal
├── rpc/
│   ├── protocol.rs             # MethodId, RpcStatus, RPC_MAGIC
│   ├── server.rs               # RpcServer — dispatcher de méthodes
│   ├── client.rs               # RpcClient — stub + retry
│   └── timeout.rs              # RpcTimeout — fn pointer injectée
└── stats/
    └── counters.rs             # IPC_STATS — AtomicU64 throughput/latences
```

---

## Initialisation

```rust
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32)
```

Doit être appelée **une seule fois** au boot, après l'initialisation de `memory/` et avant tout autre usage IPC. Elle initialise :

1. Le pool SHM (`shared_memory::pool::init_shm_pool`)
2. La structure NUMA (`shared_memory::numa_aware::numa_init`)
3. Les compteurs de statistiques globaux

Voir [INIT.md](INIT.md) pour le séquençage complet.

---

## Performance — cibles de conception

| Métrique | Cible |
|---|---|
| Latence petit message (< 240 B) | 500 – 700 cycles |
| Débit zero-copy (SHM) | > 100 M msgs/s |
| Débit streaming (SHM + DMA) | > 50 GB/s |
| Allocation SHM (pool) | < 100 ns |
| Fast IPC (ASM path) | contourne le syscall |

---

## Constantes clés

| Constante | Valeur | Rôle |
|---|---|---|
| `MAX_MSG_SIZE` | 240 octets | Seuil inline/zero-copy |
| `RING_SIZE` | 16 | Slots par ring statique |
| `RING_MASK` | 15 | Masque pour accès indexés |
| `IPC_VERSION` | 1 | Version du protocole |
| `IPC_MAX_CHANNELS` | 4 096 | Canaux simultanés max |
| `IPC_MAX_ENDPOINTS` | 1 024 | Endpoints enregistrés max |
| `IPC_MAX_PROCESSES` | 512 | Processus IPC simultanés max |
| `MSG_HEADER_MAGIC` | 0x1FCF_07E0 | Validité de l'en-tête BdB |
| `SYNC_CHANNEL_TIMEOUT_NS` | 100 ms | Timeout canal synchrone |

---

## Règles de conformité

Toutes les règles de conception obligatoires sont listées dans [RULES.md](RULES.md).

---

## Index de la documentation

| Fichier | Contenu |
|---|---|
| [README.md](README.md) | Ce fichier — vue d'ensemble |
| [RULES.md](RULES.md) | Les 8 règles IPC obligatoires |
| [API.md](API.md) | Surface publique complète |
| [INIT.md](INIT.md) | Séquence de boot et `ipc_init()` |
| [ring_buffers.md](ring_buffers.md) | SPSC, MPMC, Fusion, ZeroCopy, Slot, Batch |
| [channels.md](channels.md) | Canaux sync/async/MPMC/broadcast/typed/streaming |
| [shared_memory.md](shared_memory.md) | SHM : pages, pool, allocateur, NUMA |
| [endpoints.md](endpoints.md) | Endpoints nommés et cycle de vie |
| [sync_primitives.md](sync_primitives.md) | Futex, wait queue, event, barrier, rendezvous |
| [message_system.md](message_system.md) | Builder, sérialiseur, routeur, priorité |
| [rpc.md](rpc.md) | RPC serveur/client/protocole/timeout |
| [capability_bridge.md](capability_bridge.md) | Délégation vers `security/capability/` |
| [stats.md](stats.md) | Compteurs et métriques globaux |
