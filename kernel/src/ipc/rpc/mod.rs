// ipc/rpc/mod.rs — Module RPC IPC pour Exo-OS

pub mod client;
pub mod protocol;
pub mod raw;
pub mod server;
pub mod timeout;

// Re-exports : protocol
pub use protocol::{
    rpc_frame_size, rpc_frame_valid, MethodId, RpcCallFrame, RpcHeader, RpcReplyFrame, RpcStatus,
    MAX_RPC_PAYLOAD, METHOD_ID_INTROSPECT, METHOD_ID_PING, RPC_MAGIC, RPC_VERSION,
};

// Re-exports : timeout — types et fonctions
pub use timeout::{
    install_time_fn, now_ns, RetryState, RpcDeadline, RpcTimeout, RpcTimeoutStats,
    RpcTimeoutStatsSnapshot, TimeFn, RPC_TIMEOUT_STATS,
};
// Re-exports : timeout — constantes
pub use timeout::RPC_DEFAULT_TIMEOUT_NS;
pub use timeout::RPC_INITIAL_BACKOFF_NS;
pub use timeout::RPC_MAX_BACKOFF_NS;
pub use timeout::RPC_MAX_RETRIES;

// Re-exports : server
pub use server::{
    rpc_server_create, rpc_server_destroy, rpc_server_dispatch, rpc_server_register,
    rpc_server_stats, MethodEntry, RpcHandlerFn, RpcServer, RpcServerStats, MAX_RPC_METHODS,
    MAX_RPC_SERVERS,
};

// Re-exports : raw RPC (call_raw / parse_call / send_reply)
pub use raw::{call_raw, parse_call, send_reply, CallRequest, CALL_MAGIC, MAX_CALL_PAYLOAD};

// Re-exports : client
pub use client::{
    rpc_call, rpc_call_retry, rpc_client_create, rpc_client_destroy, rpc_client_stats, RpcClient,
    RpcClientStats, RpcResult, RpcTransportFn, MAX_RPC_CLIENTS,
};
