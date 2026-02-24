// ipc/mod.rs — Module racine IPC pour Exo-OS
//
// Ce module est le point d'entrée unique pour tout le sous-système IPC.
// Il agrège tous les sous-modules et expose les primitives nécessaires
// aux autres couches du noyau (process, scheduler, fs, drivers).
//
// Architecture du sous-système IPC :
//
//   core/          — types, constantes, séquences, transferts, fastcall asm
//   ring/          — ring buffers SPSC, MPMC, zero-copy, batch, fusion
//   capability_bridge/ — vérification et délégation de capabilities
//   endpoint/      — descripteurs, registre, connexions, lifecycle
//   channel/       — sync, async, mpmc, broadcast, typed, streaming
//   shared_memory/ — pages, pool, descripteurs, mappings, allocateur, NUMA
//   sync/          — futex, wait_queue, event, barrier, rendezvous
//   stats/         — compteurs statistiques AtomicU64 globaux
//   message/       — builder, serializer, router, priority
//   rpc/           — protocol, server, client, timeout
//
// Initialisation :
//   Appeler `ipc_init(base_phys, n_numa_nodes)` au boot.
//
// Ordre de verrouillage (regle_bonus.md) :
//   IPC locks = niveau 1 (toujours acquis avant scheduler=2, memory=3, fs=4)
//
// RÈGLE IPC-ROOT-01 : ipc_init() doit être appelé AVANT toute opération IPC.
// RÈGLE IPC-ROOT-02 : pas d'import de modules fs/ ou process/ depuis ce module.

// ---------------------------------------------------------------------------
// Sous-modules
// ---------------------------------------------------------------------------

pub mod core;
pub mod ring;
pub mod capability_bridge;
pub mod endpoint;
pub mod channel;
pub mod shared_memory;
pub mod sync;
pub mod stats;
pub mod message;
pub mod rpc;

// ---------------------------------------------------------------------------
// Re-exports principaux pour usage externe depuis kernel/
// ---------------------------------------------------------------------------

// core/ — types fondamentaux
pub use core::{
    types::{
        ChannelId, EndpointId, ProcessId, MessageFlags, MessageType, IpcError,
    },
    constants::{
        IPC_VERSION, IPC_MAX_CHANNELS, IPC_MAX_ENDPOINTS, IPC_MAX_PROCESSES,
    },
};

// stats/ — compteurs globaux
pub use stats::counters::{IPC_STATS, StatEvent, IpcStatsSnapshot};

// endpoint/ — API principale d'endpoint
pub use endpoint::{
    endpoint_create, endpoint_destroy, endpoint_listen, endpoint_close,
    do_connect as endpoint_connect, do_accept as endpoint_accept,
};

// channel/ — API channels
pub use channel::{
    sync::{sync_channel_create, sync_channel_send, sync_channel_recv, sync_channel_destroy},
};

// shared_memory/ — API SHM
pub use shared_memory::{
    allocator::{shm_alloc, shm_free},
    mapping::{shm_map, shm_unmap, register_map_hook, register_unmap_hook},
    pool::init_shm_pool,
    numa_aware::numa_init,
};

// sync/ — API synchronisation IPC
pub use sync::{
    futex::{futex_wait, futex_wake},
    event::{event_create, event_set, event_wait, event_destroy},
    barrier::{barrier_create, barrier_arrive_and_wait, barrier_destroy},
};

// message/ — API message
pub use message::{
    builder::{IpcMessage, IpcMessageBuilder, msg_data, msg_control, msg_signal},
    router::{router_add, router_remove, router_dispatch},
};

// rpc/ — API RPC
pub use rpc::{
    protocol::{MethodId, RpcStatus, RPC_MAGIC},
    server::{rpc_server_create, rpc_server_register, rpc_server_dispatch, rpc_server_destroy},
    client::{rpc_client_create, rpc_call, rpc_client_destroy},
    timeout::{RpcTimeout, install_time_fn},
};

// ---------------------------------------------------------------------------
// Initialisation globale IPC
// ---------------------------------------------------------------------------

/// Initialise le sous-système IPC.
///
/// Doit être appelée une seule fois au boot, avant tout usage IPC.
///
/// # Paramètres
/// - `shm_base_phys` : adresse physique de base du pool SHM (alignée 4K)
/// - `n_numa_nodes` : nombre de nœuds NUMA disponibles (1..=8)
///
/// # Sécurité
/// Doit être appelée en contexte de démarrage, core 0, interruptions désactivées.
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    // 1. Initialiser le pool de pages SHM
    // SAFETY: shm_base_phys doit être aligné à 4K et pointe vers de la mémoire
    //         physique réservée pour le SHM kernel (initialisé une seule fois).
    unsafe {
        shared_memory::pool::init_shm_pool(shm_base_phys);
    }

    // 2. Initialiser le gestionnaire NUMA
    // SAFETY: appelé une seule fois, n_numa_nodes validé par numa_init()
    unsafe {
        shared_memory::numa_aware::numa_init(n_numa_nodes as usize);
    }

    // 3. Enregistrer les stats IPC (reset compteurs)
    stats::counters::IPC_STATS.reset_all();

    // 4. Log minimal d'initialisation (au niveau du kernel)
    // Note : on ne peut pas utiliser log!/println! ici (no_std), mais un hook
    // d'initialisation peut être fourni par l'arch layer via les callbacks.
}

/// Retourne le snapshot des statistiques IPC globales.
pub fn ipc_stats_snapshot() -> IpcStatsSnapshot {
    IPC_STATS.snapshot()
}
