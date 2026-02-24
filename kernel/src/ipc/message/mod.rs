// ipc/message/mod.rs — Module message IPC pour Exo-OS

pub mod builder;
pub mod serializer;
pub mod router;
pub mod priority;

// Re-exports : builder
pub use builder::{
    IpcMessage,
    IpcDescriptor,
    IpcMessageBuilder,
    MAX_MSG_INLINE,
    MAX_MSG_DESCRIPTORS,
    msg_data,
    msg_control,
    msg_signal,
    msg_rpc_reply,
};

// Re-exports : serializer
pub use serializer::{
    MsgFrameHeader,
    DeserializedMessage,
    MessageFrameIter,
    FRAME_MAGIC,
    FRAME_VERSION,
    serialize_message,
    deserialize_message,
    deserialize_into_owned,
    serialize_batch,
    serialized_size,
    write_u32,
    read_u32,
    write_u64,
    read_u64,
    write_bytes,
    read_bytes,
};

// Re-exports : router
pub use router::{
    IpcRouter,
    IpcRouterStats,
    RouteEntry,
    MAX_ROUTES,
    MAX_HOPS,
    IPC_ROUTER,
    router_add,
    router_remove,
    router_lookup,
    router_resolve,
    router_dispatch,
    router_stats,
};

// Re-exports : priority
pub use priority::{
    PriorityQueue,
    PriorityQueueStats,
    PrioMsgSlot,
    PRIORITY_RT_THRESHOLD,
    PRIORITY_RT_RING_CAP,
    PRIORITY_NORMAL_RING_CAP,
    MAX_PRIORITY_QUEUES,
    prio_queue_create,
    prio_queue_enqueue,
    prio_queue_dequeue,
    prio_queue_is_empty,
    prio_queue_len,
    prio_queue_destroy,
    prio_queue_stats,
};
