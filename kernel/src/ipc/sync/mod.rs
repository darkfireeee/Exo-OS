// ipc/sync/mod.rs — Module de synchronisation IPC pour Exo-OS
//
// Ce module regroupe toutes les primitives de synchronisation IPC :
// - futex        : table de futex IPC (wait/wake/requeue atomiques)
// - wait_queue   : file d'attente IPC avec timeout et politique de réveil
// - event        : événement binaire (auto-reset/manual-reset) et sémaphore léger
// - barrier      : barrière cyclique N-threads
// - rendezvous   : rendez-vous N-voies et échange symétrique bilatéral

pub mod futex;
pub mod wait_queue;
pub mod event;
pub mod barrier;
pub mod rendezvous;
pub mod sched_hooks;

// Re-exports : futex
pub use futex::{
    FutexKey,
    WaiterState,
    FutexIpcStats,
    futex_wait,
    futex_wake,
    futex_wake_all,
    futex_cancel,
    futex_requeue,
    futex_stats,
};

// Re-exports : wait_queue
pub use wait_queue::{
    IpcWaiter,
    IpcWaitQueue,
    IpcWaitQueueStats,
    WakePolicy,
    WakeReason,
    MAX_IPC_WAITERS,
};

// Re-exports : event
pub use event::{
    IpcEvent,
    IpcEventStats,
    IpcCountingEvent,
    EventMode,
    MAX_EVENT_COUNT,
    MAX_IPC_EVENTS,
    event_create,
    event_set,
    event_clear,
    event_is_set,
    event_wait,
    event_destroy,
    event_count,
    event_stats,
};

// Re-exports : barrier
pub use barrier::{
    IpcBarrier,
    IpcBarrierStats,
    BarrierResult,
    MAX_BARRIER_PARTIES,
    MAX_IPC_BARRIERS,
    barrier_create,
    barrier_arrive_and_wait,
    barrier_arrive,
    barrier_wait_phase,
    barrier_generation,
    barrier_reset,
    barrier_destroy,
    barrier_count,
    barrier_stats,
};

// Re-exports : rendezvous
pub use rendezvous::{
    IpcRendezvous,
    IpcRendezvousStats,
    RendezvousState,
    ExchangeSlot,
    ExchangeState,
    MAX_RENDEZVOUS_PARTIES,
    MAX_RENDEZVOUS_ENTRIES,
    MAX_EXCHANGE_SLOTS,
    MAX_EXCHANGE_SIZE,
    rendezvous_create,
    rendezvous_meet,
    rendezvous_rearm,
    rendezvous_destroy,
    rendezvous_arrived,
    rendezvous_stats,
    exchange_create,
    exchange_swap,
    exchange_destroy,
};

// Re-exports : sched_hooks
pub use sched_hooks::{
    install_block_hook,
    block_current,
    wake_thread,
    hooks_installed,
    current_tid as sched_current_tid,
    BlockFn as SchedBlockFn,
};
