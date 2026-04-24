// ipc/channel/mod.rs — Module racine des canaux IPC pour Exo-OS
//
// Ce module regroupe tous les types de canaux disponibles :
//   - SyncChannel    : rendezvous (émetteur bloqué jusqu'à réception)
//   - AsyncChannel   : non-bloquant avec notification par waker
//   - MpmcChannel    : multi-producteurs / multi-consommateurs lock-free
//   - BroadcastChannel : diffusion one-to-many (pub/sub)
//   - TypedChannel<T> : canal générique typé (T: Copy)
//   - StreamChannel  : streaming zero-copy pour grands volumes

pub mod r#async;
pub mod broadcast;
pub mod mpmc;
pub mod raw;
pub mod streaming;
pub mod sync;
pub mod typed;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Canal brut (raw mailbox) — bridge syscall ↔ IPC
pub use raw::{
    mailbox_close,
    mailbox_open,
    mailbox_open_count,
    raw_stats_snapshot,
    recv_raw,
    recv_raw_checked,
    send_raw,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    send_raw_checked,
    RawSlotStats,
    MAX_RAW_SLOTS,
};

// Canal synchrone (rendezvous)
pub use sync::{
    sync_channel_close,
    sync_channel_count,
    sync_channel_create,
    sync_channel_destroy,
    sync_channel_recv,
    sync_channel_recv_checked,
    sync_channel_send,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    sync_channel_send_checked,
    RendezVousState,
    SyncChannel,
    SyncChannelSnapshot,
    SyncChannelStats,
    SyncSlot,
    SYNC_CHANNEL_TABLE_SIZE,
    SYNC_INLINE_SIZE,
};

// Canal asynchrone
pub use r#async::{
    async_channel_count, async_channel_create, async_channel_destroy, async_channel_register_waker,
    async_channel_send, async_channel_try_recv, async_channel_unregister_waker, AsyncChannel,
    AsyncChannelStats, AsyncChannelStatsSnapshot, AsyncWaker, WakeFn, WakerTable, MAX_ASYNC_WAKERS,
};

// Canal MPMC
pub use mpmc::{
    mpmc_channel_count,
    mpmc_channel_create,
    mpmc_channel_destroy,
    mpmc_channel_recv,
    mpmc_channel_recv_checked,
    mpmc_channel_send,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    mpmc_channel_send_checked,
    MpmcChannel,
    MpmcStats,
    MpmcStatsSnapshot,
    OverflowPolicy,
    MPMC_CHANNEL_TABLE_SIZE,
};

// Canal broadcast
pub use broadcast::{
    broadcast_create,
    broadcast_destroy,
    broadcast_publish,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    broadcast_publish_checked,
    broadcast_recv,
    broadcast_recv_checked,
    broadcast_subscribe,
    broadcast_subscribe_checked,
    broadcast_unsubscribe,
    BroadcastChannel,
    BroadcastStats,
    BroadcastStatsSnapshot,
    SubscriberId,
    SubscriberSlot,
    MAX_BROADCAST_SUBSCRIBERS,
    SUBSCRIBER_INVALID,
};

// Canal typé générique
pub use typed::{
    typed_channel_count, typed_channel_destroy, TypedChannel, TypedChannelInner, TypedChannelTable,
    TypedReceiver, TypedSender, MAX_TYPED_VALUE_SIZE,
};

// Canal streaming
pub use streaming::{
    stream_alloc_buffer, stream_channel_create, stream_channel_destroy, stream_pop, stream_push,
    stream_release_buffer, StreamBuffer, StreamChannel, StreamGranule, StreamPool, StreamStats,
    StreamStatsSnapshot, STREAM_CHANNEL_TABLE_SIZE, STREAM_POOL_SIZE,
};
