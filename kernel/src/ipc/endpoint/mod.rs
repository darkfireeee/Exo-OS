// kernel/src/ipc/endpoint/mod.rs
//
// Module endpoint IPC.

pub mod descriptor;
pub mod registry;
pub mod connection;
pub mod lifecycle;

pub use descriptor::{EndpointDesc, EndpointName, EndpointState, PendingConnection};
pub use registry::{
    NamedEndpointRegistry, ENDPOINT_REGISTRY,
    register_endpoint, lookup_endpoint, unregister_endpoint,
};
pub use connection::{HandshakeMsg, ActiveConnection, ConnectResult, AcceptResult, do_connect, do_accept};
pub use lifecycle::{endpoint_create, endpoint_listen, endpoint_close, endpoint_destroy, active_endpoint_count};
