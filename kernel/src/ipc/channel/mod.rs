// ipc/channel/mod.rs — Root module for IPC channels for Exo-OS
//
// This module groups all available channel types:
//   - SyncChannel      : rendezvous (sender blocked until reception)
//   - AsyncChannel     : non-blocking with waker notification
//   - MpmcChannel      : multi-producer / multi-consumer lock-free
//   - BroadcastChannel : one-to-many broadcast (pub/sub)
//   - TypedChannel<T>  : generic typed channel (T: Copy)
//   - StreamChannel    : zero-copy streaming for large volumes

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

// Raw channel (raw mailbox) — syscall <-> IPC bridge
pub use raw::{
    mailbox_close,
    mailbox_open,
    mailbox_open_count,
    raw_stats_snapshot,
    recv_raw,
    recv_raw_checked,
    send_raw,
    // IPC-04 (v6) — cap-checked variants for syscall layer
    send_raw_checked,
    RawSlotStats,
    MAX_RAW_SLOTS,
};

// Synchronous channel (rendezvous)
pub use sync::{
    sync_channel_close,
    sync_channel_count,
    sync_channel_create,
    sync_channel_destroy,
    sync_channel_recv,
    sync_channel_recv_checked,
    sync_channel_send,
    // IPC-04 (v6) — cap-checked variants for syscall layer
    sync_channel_send_checked,
    RendezVousState,
    SyncChannel,
    SyncChannelSnapshot,
    SyncChannelStats,
    SyncSlot,
    SYNC_CHANNEL_TABLE_SIZE,
    SYNC_INLINE_SIZE,
};

// Asynchronous channel
pub use r#async::{
    async_channel_count, async_channel_create, async_channel_destroy, async_channel_register_waker,
    async_channel_send, async_channel_try_recv, async_channel_unregister_waker, AsyncChannel,
    AsyncChannelStats, AsyncChannelStatsSnapshot, AsyncWaker, WakeFn, WakerTable, MAX_ASYNC_WAKERS,
};

// MPMC channel
pub use mpmc::{
    mpmc_channel_count,
    mpmc_channel_create,
    mpmc_channel_destroy,
    mpmc_channel_recv,
    mpmc_channel_recv_checked,
    mpmc_channel_send,
    // IPC-04 (v6) — cap-checked variants for syscall layer
    mpmc_channel_send_checked,
    MpmcChannel,
    MpmcStats,
    MpmcStatsSnapshot,
    OverflowPolicy,
    MPMC_CHANNEL_TABLE_SIZE,
};

// Broadcast channel
pub use broadcast::{
    broadcast_create,
    broadcast_destroy,
    broadcast_publish,
    // IPC-04 (v6) — cap-checked variants for syscall layer
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

// Generic typed channel
pub use typed::{
    typed_channel_count, typed_channel_destroy, TypedChannel, TypedChannelInner, TypedChannelTable,
    TypedReceiver, TypedSender, MAX_TYPED_VALUE_SIZE,
};

// Streaming channel
pub use streaming::{
    stream_alloc_buffer, stream_channel_create, stream_channel_destroy, stream_pop, stream_push,
    stream_release_buffer, StreamBuffer, StreamChannel, StreamGranule, StreamPool, StreamStats,
    StreamStatsSnapshot, STREAM_CHANNEL_TABLE_SIZE, STREAM_POOL_SIZE,
};
