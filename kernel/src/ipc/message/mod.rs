// ipc/message/mod.rs — Module message IPC pour Exo-OS

pub mod builder;
pub mod priority;
pub mod router;
pub mod serializer;

// Re-exports : builder
pub use builder::{
    msg_control, msg_data, msg_rpc_reply, msg_signal, IpcDescriptor, IpcMessage, IpcMessageBuilder,
    MAX_MSG_DESCRIPTORS, MAX_MSG_INLINE,
};

// Re-exports : serializer
pub use serializer::{
    deserialize_into_owned, deserialize_message, read_bytes, read_u32, read_u64, serialize_batch,
    serialize_message, serialized_size, write_bytes, write_u32, write_u64, DeserializedMessage,
    MessageFrameIter, MsgFrameHeader, FRAME_MAGIC, FRAME_VERSION,
};

// Re-exports : router
pub use router::{
    router_add, router_dispatch, router_lookup, router_remove, router_resolve, router_stats,
    IpcRouter, IpcRouterStats, RouteEntry, IPC_ROUTER, MAX_HOPS, MAX_ROUTES,
};

// Re-exports : priority
pub use priority::{
    prio_queue_create, prio_queue_dequeue, prio_queue_destroy, prio_queue_enqueue,
    prio_queue_is_empty, prio_queue_len, prio_queue_stats, PrioMsgSlot, PriorityQueue,
    PriorityQueueStats, MAX_PRIORITY_QUEUES, PRIORITY_NORMAL_RING_CAP, PRIORITY_RT_RING_CAP,
    PRIORITY_RT_THRESHOLD,
};
