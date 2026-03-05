// ipc/channel/mod.rs — Module racine des canaux IPC pour Exo-OS
//
// Ce module regroupe tous les types de canaux disponibles :
//   - SyncChannel    : rendezvous (émetteur bloqué jusqu'à réception)
//   - AsyncChannel   : non-bloquant avec notification par waker
//   - MpmcChannel    : multi-producteurs / multi-consommateurs lock-free
//   - BroadcastChannel : diffusion one-to-many (pub/sub)
//   - TypedChannel<T> : canal générique typé (T: Copy)
//   - StreamChannel  : streaming zero-copy pour grands volumes

pub mod raw;
pub mod sync;
pub mod r#async;
pub mod mpmc;
pub mod broadcast;
pub mod typed;
pub mod streaming;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Canal brut (raw mailbox) — bridge syscall ↔ IPC
pub use raw::{
    send_raw, recv_raw, mailbox_open, mailbox_close,
    mailbox_open_count, raw_stats_snapshot,
    MAX_RAW_SLOTS, RawSlotStats,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    send_raw_checked, recv_raw_checked,
};

// Canal synchrone (rendezvous)
pub use sync::{
    SyncChannel, SyncChannelStats, SyncChannelSnapshot, SyncSlot, RendezVousState,
    SYNC_CHANNEL_TABLE_SIZE, SYNC_INLINE_SIZE,
    sync_channel_create, sync_channel_send, sync_channel_recv,
    sync_channel_close, sync_channel_destroy, sync_channel_count,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    sync_channel_send_checked, sync_channel_recv_checked,
};

// Canal asynchrone
pub use r#async::{
    AsyncChannel, AsyncChannelStats, AsyncChannelStatsSnapshot,
    AsyncWaker, WakerTable, WakeFn, MAX_ASYNC_WAKERS,
    async_channel_create, async_channel_register_waker,
    async_channel_unregister_waker, async_channel_send,
    async_channel_try_recv, async_channel_destroy, async_channel_count,
};

// Canal MPMC
pub use mpmc::{
    MpmcChannel, MpmcStats, MpmcStatsSnapshot, OverflowPolicy,
    MPMC_CHANNEL_TABLE_SIZE,
    mpmc_channel_create, mpmc_channel_send, mpmc_channel_recv,
    mpmc_channel_destroy, mpmc_channel_count,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    mpmc_channel_send_checked, mpmc_channel_recv_checked,
};

// Canal broadcast
pub use broadcast::{
    BroadcastChannel, BroadcastStats, BroadcastStatsSnapshot,
    SubscriberSlot, SubscriberId, SUBSCRIBER_INVALID,
    MAX_BROADCAST_SUBSCRIBERS,
    broadcast_create, broadcast_subscribe, broadcast_unsubscribe,
    broadcast_publish, broadcast_recv, broadcast_destroy,
    // IPC-04 (v6) — variantes cap-checked pour la couche syscall
    broadcast_publish_checked, broadcast_subscribe_checked, broadcast_recv_checked,
};

// Canal typé générique
pub use typed::{
    TypedChannel, TypedSender, TypedReceiver, TypedChannelInner,
    TypedChannelTable, MAX_TYPED_VALUE_SIZE,
    typed_channel_destroy, typed_channel_count,
};

// Canal streaming
pub use streaming::{
    StreamChannel, StreamPool, StreamBuffer, StreamStats, StreamStatsSnapshot,
    StreamGranule, STREAM_POOL_SIZE, STREAM_CHANNEL_TABLE_SIZE,
    stream_channel_create, stream_alloc_buffer, stream_push,
    stream_pop, stream_release_buffer, stream_channel_destroy,
};
