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
//   IPC locks = niveau 4 (après Memory→Scheduler→Security)
// IPC locks — niveau 4 dans l'ordre canonique ExoOS :
// Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5)
// NE JAMAIS acquérir un lock Memory/Scheduler dans un contexte IPC verrouillé.
//
//
// RÈGLE IPC-ROOT-01 : ipc_init() doit être appelé AVANT toute opération IPC.
// RÈGLE IPC-ROOT-02 : pas d'import de modules fs/ ou process/ depuis ce module.

// ---------------------------------------------------------------------------
// Sous-modules
// ---------------------------------------------------------------------------

pub mod channel;
pub mod core;
pub mod endpoint;
pub mod message;
pub mod ring;
pub mod rpc;
pub mod shared_memory;
pub mod stats;
pub mod sync;

// ---------------------------------------------------------------------------
// Re-exports principaux pour usage externe depuis kernel/
// ---------------------------------------------------------------------------

// core/ — types fondamentaux
pub use core::{
    constants::{IPC_MAX_CHANNELS, IPC_MAX_ENDPOINTS, IPC_MAX_PROCESSES, IPC_VERSION},
    types::{ChannelId, EndpointId, IpcError, MessageFlags, MessageType, ProcessId},
};

// stats/ — compteurs globaux
pub use stats::counters::{IpcStatsSnapshot, StatEvent, IPC_STATS};

// endpoint/ — API principale d'endpoint
pub use endpoint::{
    do_accept as endpoint_accept, do_connect as endpoint_connect, endpoint_close, endpoint_create,
    endpoint_destroy, endpoint_listen,
};

// channel/ — API channels
pub use channel::sync::{
    sync_channel_create, sync_channel_destroy, sync_channel_recv, sync_channel_send,
};

// shared_memory/ — API SHM
pub use shared_memory::{
    allocator::{shm_alloc, shm_free},
    mapping::{register_map_hook, register_unmap_hook, shm_map, shm_unmap},
    numa_aware::numa_init,
    pool::init_shm_pool,
};

// sync/ — API synchronisation IPC
pub use sync::{
    barrier::{barrier_arrive_and_wait, barrier_create, barrier_destroy},
    event::{event_create, event_destroy, event_set, event_wait},
    futex::{
        futex_cancel, futex_requeue, futex_stats, futex_wait, futex_wake, futex_wake_all,
        FutexIpcStats, FutexKey, WaiterState,
    },
};

// message/ — API message
pub use message::{
    builder::{msg_control, msg_data, msg_signal, IpcMessage, IpcMessageBuilder},
    router::{router_add, router_dispatch, router_remove},
};

// rpc/ — API RPC
pub use rpc::{
    client::{rpc_call, rpc_client_create, rpc_client_destroy},
    protocol::{MethodId, RpcStatus, RPC_MAGIC},
    server::{rpc_server_create, rpc_server_destroy, rpc_server_dispatch, rpc_server_register},
    timeout::{install_time_fn, RpcTimeout},
};

/// Envoie une notification IRQ bornée et non bloquante à un endpoint driver.
///
/// Payload canonique :
/// - octet 0  : numéro IRQ
/// - octets 1..8 : wave generation little-endian
pub fn send_irq_notification(
    endpoint: &exo_types::IpcEndpoint,
    irq: u8,
    wave_gen: u64,
) -> Result<(), IpcError> {
    let endpoint_code = ((endpoint.pid as u64) << 32) | endpoint.chan_idx as u64;
    let endpoint_id = EndpointId::new(endpoint_code).ok_or(IpcError::NullEndpoint)?;

    let mut payload = [0u8; 9];
    payload[0] = irq;
    payload[1..].copy_from_slice(&wave_gen.to_le_bytes());

    channel::raw::try_send_raw_nowait(endpoint_id, &payload).map(|_| ())
}

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
    // SAFETY: shm_base_phys aligné 4K, mémoire physique réservée SHM, appelé une seule fois au boot.
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

// ---------------------------------------------------------------------------
// Hooks d'intégration — scheduler et VMM
// ---------------------------------------------------------------------------

/// Connecte le hook de blocage réel du scheduler à l'IPC.
///
/// Doit être appelé UNE SEULE FOIS, après `scheduler::init()` et AVANT tout
/// appel IPC bloquant (futex_wait, sync_channel_send/recv, wait_queue).
///
/// Sans cet appel, les primitives IPC tombent en mode spin-poll de secours
/// (dégradé, acceptable en monocœur ou lors des tests unitaires).
///
/// `block_fn` : suspend le thread courant jusqu'à réveil explicite.
///              Fournie par `scheduler::block_current_thread`.
pub fn ipc_install_scheduler_hooks(block_fn: sync::sched_hooks::BlockFn) {
    sync::sched_hooks::install_block_hook(block_fn);
}

/// Connecte les hooks VMM de mappage/démappage SHM à l'IPC.
///
/// Doit être appelé UNE SEULE FOIS, après que le gestionnaire de mémoire
/// virtuelle est opérationnel et que les tables de pages des processus
/// sont gérées.
///
/// Sans ces hooks, `shm_map()` opère en mode simulé (virt = phys) —
/// acceptable en dev/test mono-processus sans isolation d'espace d'adressage.
///
/// - `map_page_fn`   : `unsafe fn(phys: u64, virt: u64, flags: u32, pid: u32) -> i32`
///                     0 = succès, non-zéro = erreur.
/// - `unmap_page_fn` : `unsafe fn(virt: u64, pid: u32) -> i32`
pub fn ipc_install_vmm_hooks(
    map_page_fn: shared_memory::mapping::MapPageFn,
    unmap_page_fn: shared_memory::mapping::UnmapPageFn,
) {
    shared_memory::mapping::register_map_hook(map_page_fn);
    shared_memory::mapping::register_unmap_hook(unmap_page_fn);
}
