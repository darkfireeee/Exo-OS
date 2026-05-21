// ipc/sync/mod.rs — Module de synchronisation IPC pour Exo-OS
//
// Ce module regroupe toutes les primitives de synchronisation IPC :
// - futex        : table de futex IPC (wait/wake/requeue atomiques)
// - wait_queue   : file d'attente IPC avec timeout et politique de réveil
// - event        : événement binaire (auto-reset/manual-reset) et sémaphore léger
// - barrier      : barrière cyclique N-threads
// - rendezvous   : rendez-vous N-voies et échange symétrique bilatéral

pub mod barrier;
pub mod event;
pub mod futex;
pub mod rendezvous;
pub mod sched_hooks;
pub mod wait_queue;

/// Default bounded IPC wait budget used by audit-facing synchronization APIs.
pub const IPC_TIMEOUT_DEFAULT_NS: u64 = 1_000_000;
/// Maximum bounded IPC wait budget for the IPC synchronization layer.
pub const IPC_TIMEOUT_MAX_NS: u64 = 10_000_000;
/// Live wait queues use zero as the no-deadline sentinel.
pub const IPC_TIMEOUT_ZERO: u64 = 0;

const _: () = assert!(
    IPC_TIMEOUT_DEFAULT_NS <= IPC_TIMEOUT_MAX_NS,
    "IPC default timeout exceeds its bounded maximum"
);

// Re-exports : futex
pub use futex::{
    futex_cancel, futex_requeue, futex_stats, futex_wait, futex_wake, futex_wake_all,
    FutexIpcStats, FutexKey, WaiterState,
};

// Re-exports : wait_queue
pub use wait_queue::{
    IpcWaitQueue, IpcWaitQueueStats, IpcWaiter, WakePolicy, WakeReason, MAX_IPC_WAITERS,
};

// Re-exports : event
pub use event::{
    event_clear, event_count, event_create, event_destroy, event_is_set, event_set, event_stats,
    event_wait, EventMode, IpcCountingEvent, IpcEvent, IpcEventStats, MAX_EVENT_COUNT,
    MAX_IPC_EVENTS,
};

// Re-exports : barrier
pub use barrier::{
    barrier_arrive, barrier_arrive_and_wait, barrier_count, barrier_create, barrier_destroy,
    barrier_generation, barrier_reset, barrier_stats, barrier_wait_phase, BarrierResult,
    IpcBarrier, IpcBarrierStats, MAX_BARRIER_PARTIES, MAX_IPC_BARRIERS,
};

// Re-exports : rendezvous
pub use rendezvous::{
    exchange_create, exchange_destroy, exchange_swap, rendezvous_arrived, rendezvous_create,
    rendezvous_destroy, rendezvous_meet, rendezvous_rearm, rendezvous_stats, ExchangeSlot,
    ExchangeState, IpcRendezvous, IpcRendezvousStats, RendezvousState, MAX_EXCHANGE_SIZE,
    MAX_EXCHANGE_SLOTS, MAX_RENDEZVOUS_ENTRIES, MAX_RENDEZVOUS_PARTIES,
};

// Re-exports : sched_hooks
pub use sched_hooks::{
    block_current, current_tid as sched_current_tid, hooks_installed, install_block_hook,
    wake_thread, BlockFn as SchedBlockFn,
};
