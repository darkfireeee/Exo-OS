// ipc/rpc/mod.rs — Module RPC IPC pour Exo-OS

pub mod raw;
pub mod protocol;
pub mod timeout;
pub mod server;
pub mod client;

// Re-exports : protocol
pub use protocol::{
    RpcHeader,
    RpcCallFrame,
    RpcReplyFrame,
    RpcStatus,
    MethodId,
    RPC_MAGIC,
    RPC_VERSION,
    MAX_RPC_PAYLOAD,
    METHOD_ID_PING,
    METHOD_ID_INTROSPECT,
    rpc_frame_size,
    rpc_frame_valid,
};

// Re-exports : timeout — types et fonctions
pub use timeout::{
    RpcTimeout,
    RpcDeadline,
    RetryState,
    RpcTimeoutStats,
    RpcTimeoutStatsSnapshot,
    TimeFn,
    RPC_TIMEOUT_STATS,
    install_time_fn,
    now_ns,
};
// Re-exports : timeout — constantes
pub use timeout::RPC_DEFAULT_TIMEOUT_NS;
pub use timeout::RPC_MAX_RETRIES;
pub use timeout::RPC_INITIAL_BACKOFF_NS;
pub use timeout::RPC_MAX_BACKOFF_NS;

// Re-exports : server
pub use server::{
    RpcServer,
    RpcServerStats,
    MethodEntry,
    RpcHandlerFn,
    MAX_RPC_METHODS,
    MAX_RPC_SERVERS,
    rpc_server_create,
    rpc_server_register,
    rpc_server_dispatch,
    rpc_server_destroy,
    rpc_server_stats,
};

// Re-exports : raw RPC (call_raw / parse_call / send_reply)
pub use raw::{
    call_raw, parse_call, send_reply, CallRequest,
    MAX_CALL_PAYLOAD, CALL_MAGIC,
};

// Re-exports : client
pub use client::{
    RpcClient,
    RpcClientStats,
    RpcResult,
    RpcTransportFn,
    MAX_RPC_CLIENTS,
    rpc_client_create,
    rpc_call,
    rpc_call_retry,
    rpc_client_destroy,
    rpc_client_stats,
};
