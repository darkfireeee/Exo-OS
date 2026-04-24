// kernel/src/ipc/endpoint/mod.rs
//
// Module endpoint IPC.

pub mod connection;
pub mod descriptor;
pub mod lifecycle;
pub mod registry;

pub use connection::{
    do_accept, do_connect, AcceptResult, ActiveConnection, ConnectResult, HandshakeMsg,
};
pub use descriptor::{EndpointDesc, EndpointName, EndpointState, PendingConnection};
pub use lifecycle::{
    active_endpoint_count, endpoint_close, endpoint_create, endpoint_destroy, endpoint_listen,
};
pub use registry::{
    lookup_endpoint, register_endpoint, unregister_endpoint, NamedEndpointRegistry,
    ENDPOINT_REGISTRY,
};
