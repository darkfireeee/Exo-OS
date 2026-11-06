// ipc/mod.rs — Root IPC module for Exo-OS
//
// This module is the single entry point for the entire IPC subsystem.
// It aggregates all submodules and exposes the primitives needed
// by other kernel layers (process, scheduler, fs, drivers).
//
// IPC subsystem architecture:
//
//   core/          — types, constants, sequences, transfers, fastcall asm
//   ring/          — ring buffers SPSC, MPMC, zero-copy, batch, fusion
//   endpoint/      — descriptors, registry, connections, lifecycle
//   channel/       — sync, async, mpmc, broadcast, typed, streaming
//   shared_memory/ — pages, pool, descriptors, mappings, allocator, NUMA
//   sync/          — futex, wait_queue, event, barrier, rendezvous
//   stats/         — global AtomicU64 statistical counters
//   message/       — builder, serializer, router, priority
//   rpc/           — protocol, server, client, timeout
//
// Initialization:
//   Call `ipc_init(base_phys, n_numa_nodes)` at boot.
//
// Lock ordering (regle_bonus.md):
//   IPC locks = level 4 (after Memory -> Scheduler -> Security)
// IPC locks — level 4 in the ExoOS canonical order:
// Memory(1) -> Scheduler(2) -> Security(3) -> IPC(4) -> FS(5)
// NEVER acquire a Memory/Scheduler lock within a locked IPC context.
//
//
// RULE IPC-ROOT-01: ipc_init() must be called BEFORE any IPC operation.
// RULE IPC-ROOT-02: no imports of fs/ or process/ modules from this module.

// ---------------------------------------------------------------------------
// Submodules
// ---------------------------------------------------------------------------

pub mod capability_bridge;
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
// Main re-exports for external usage from kernel/
// ---------------------------------------------------------------------------

use crate::rust_core::sync::atomic::{AtomicU8, Ordering};
use exo_types::IpcEndpoint;

// core/ — fundamental types
pub use core::{
    constants::{IPC_MAX_CHANNELS, IPC_MAX_ENDPOINTS, IPC_MAX_PROCESSES, IPC_VERSION},
    types::{ChannelId, EndpointId, IpcError, MessageFlags, MessageType, ProcessId},
};

// stats/ — global counters
pub use stats::counters::{IpcStatsSnapshot, StatEvent, IPC_STATS};

// endpoint/ — main endpoint API
pub use endpoint::{
    do_accept as endpoint_accept, do_connect as endpoint_connect, endpoint_close, endpoint_create,
    endpoint_destroy, endpoint_listen,
};

// capability_bridge/ — compat toward security::access_control
pub use capability_bridge::{
    capability_to_ipc_error, check_channel_access, check_endpoint_access, check_shm_access,
};

// channel/ — channels API
pub use channel::sync::{
    sync_channel_create, sync_channel_destroy, sync_channel_recv, sync_channel_send,
};

// shared_memory/ — SHM API
pub use shared_memory::{
    allocator::{shm_alloc, shm_free},
    mapping::{
        register_map_hook, register_unmap_hook, shm_map, shm_unmap, shm_unmap_for_process,
        vmm_hooks_ready,
    },
    numa_aware::numa_init,
    pool::init_shm_pool,
};

// sync/ — IPC synchronization API
pub use sync::{
    barrier::{barrier_arrive_and_wait, barrier_create, barrier_destroy},
    event::{event_create, event_destroy, event_set, event_wait},
    futex::{
        futex_cancel, futex_requeue, futex_stats, futex_wait, futex_wake, futex_wake_all,
        FutexIpcStats, FutexKey, WaiterState,
    },
};

// message/ — message API
pub use message::{
    builder::{msg_control, msg_data, msg_signal, IpcMessage, IpcMessageBuilder},
    router::{router_add, router_dispatch, router_remove},
};

// rpc/ — RPC API
pub use rpc::{
    client::{rpc_call, rpc_client_create, rpc_client_destroy},
    protocol::{MethodId, RpcStatus, RPC_MAGIC},
    server::{rpc_server_create, rpc_server_destroy, rpc_server_dispatch, rpc_server_register},
    timeout::{install_time_fn, RpcTimeout},
};

/// Sends a bounded, non-blocking IRQ notification to a driver endpoint.
///
/// Canonical payload:
/// - byte 0   : IRQ number
/// - bytes 1..8: wave generation little-endian
pub fn send_irq_notification(
    endpoint: &IpcEndpoint,
    irq: u8,
    wave_gen: u64,
) -> Result<(), IpcError> {
    if endpoint.pid == 0 {
        return Err(IpcError::InvalidEndpoint);
    }
    let endpoint_code = ((endpoint.pid as u64) << 32) | endpoint.chan_idx as u64;
    let endpoint_id = EndpointId::new(endpoint_code).ok_or(IpcError::NullEndpoint)?;

    let mut payload = [0u8; 9];
    payload[0] = irq;
    payload[1..].copy_from_slice(&wave_gen.to_le_bytes());

    channel::raw::try_send_raw_nowait(endpoint_id, &payload).map(|_| ())
}

// ---------------------------------------------------------------------------
// Global IPC initialization
// ---------------------------------------------------------------------------

const IPC_INIT_DONE: u8 = 1 << 0;
const IPC_SCHED_HOOKS_DONE: u8 = 1 << 1;
const IPC_VMM_HOOKS_DONE: u8 = 1 << 2;

static IPC_INIT_STATE: AtomicU8 = AtomicU8::new(0);

/// Initializes the IPC subsystem.
///
/// Must be called once at boot, before any IPC usage.
///
/// # Parameters
/// - `shm_base_phys`: base physical address of the SHM pool (4K-aligned)
/// - `n_numa_nodes`: number of available NUMA nodes (1..=8)
///
/// # Safety
/// Must be called in startup context, core 0, interrupts disabled.
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    // Re-entrancy guard: do not reinitialize pools and counters if already done.
    if IPC_INIT_STATE.load(Ordering::Acquire) & IPC_INIT_DONE != 0 {
        return;
    }

    // 1. Initialize the SHM page pool
    // SAFETY: shm_base_phys 4K-aligned, reserved physical memory for SHM, called once at boot.
    unsafe {
        shared_memory::pool::init_shm_pool(shm_base_phys);
    }

    // 2. Initialize the NUMA manager
    // SAFETY: called once, n_numa_nodes validated by numa_init()
    unsafe {
        shared_memory::numa_aware::numa_init(n_numa_nodes as usize);
    }

    // 3. Register IPC stats (reset counters)
    stats::counters::IPC_STATS.reset_all();

    // 4. Publish the endpoint registry, static but explicitly initialized.
    unsafe {
        endpoint::registry::init_endpoint_registry();
    }

    // 5. Connect the reverse SHM provider toward memory::virtual.
    shared_memory::memory_bridge::register_with_memory();

    // 6. Minimal initialization log (kernel level)
    // Note: cannot use log!/println! here (no_std), but an init hook
    // can be provided by the arch layer via callbacks.
    IPC_INIT_STATE.fetch_or(IPC_INIT_DONE, Ordering::Release);
}

/// Returns a snapshot of global IPC statistics.
pub fn ipc_stats_snapshot() -> IpcStatsSnapshot {
    IPC_STATS.snapshot()
}

#[inline]
pub fn ipc_initialized() -> bool {
    IPC_INIT_STATE.load(Ordering::Acquire) & IPC_INIT_DONE != 0
}

#[inline]
pub fn ipc_scheduler_hooks_ready() -> bool {
    IPC_INIT_STATE.load(Ordering::Acquire) & IPC_SCHED_HOOKS_DONE != 0
}

#[inline]
pub fn ipc_vmm_hooks_ready() -> bool {
    IPC_INIT_STATE.load(Ordering::Acquire) & IPC_VMM_HOOKS_DONE != 0
}

// ---------------------------------------------------------------------------
// Integration hooks — scheduler and VMM
// ---------------------------------------------------------------------------

/// Connects the real scheduler blocking hook to IPC.
///
/// Must be called ONCE, after `scheduler::init()` and BEFORE any
/// blocking IPC call (futex_wait, sync_channel_send/recv, wait_queue).
///
/// Without this call, IPC primitives fall back to emergency spin-poll mode
/// (degraded, acceptable on uniprocessor or during unit tests).
///
/// `block_fn`: suspends current thread until explicit wakeup.
///             Provided by `scheduler::block_current_thread`.
pub fn ipc_install_scheduler_hooks(block_fn: sync::sched_hooks::BlockFn) {
    sync::sched_hooks::install_block_hook(block_fn);
    IPC_INIT_STATE.fetch_or(IPC_SCHED_HOOKS_DONE, Ordering::Release);
}

/// Connects the VMM SHM mapping/unmapping hooks to IPC.
///
/// Must be called ONCE, after the virtual memory manager is operational
/// and process page tables are managed.
///
/// Without these hooks, `shm_map()` returns `MappingFailed` in production.
/// The simulated virt=phys mode exists only with the `dev_no_vmm` feature.
///
/// - `map_page_fn`   : `unsafe fn(phys: u64, virt: u64, flags: u32, pid: u32) -> i32`
///                     0 = success, non-zero = error.
/// - `unmap_page_fn` : `unsafe fn(virt: u64, pid: u32) -> i32`
pub fn ipc_install_vmm_hooks(
    map_page_fn: shared_memory::mapping::MapPageFn,
    unmap_page_fn: shared_memory::mapping::UnmapPageFn,
) {
    shared_memory::mapping::register_map_hook(map_page_fn);
    shared_memory::mapping::register_unmap_hook(unmap_page_fn);
    IPC_INIT_STATE.fetch_or(IPC_VMM_HOOKS_DONE, Ordering::Release);
}
