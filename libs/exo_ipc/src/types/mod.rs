// libs/exo_ipc/src/types/mod.rs
//! Types fondamentaux pour IPC

pub mod capability;
pub mod endpoint;
pub mod error;
pub mod message;

// Réexportations
pub use capability::{Capability, Rights, IpcDescriptor, CapabilityId};
#[allow(deprecated)]
pub use capability::Permissions;  // Deprecated alias pour compatibilité
pub use endpoint::{Endpoint, EndpointId, EndpointType, IpcAddress};
pub use error::{IpcError, IpcResult, RecvError, SendError};
pub use message::{
    Message, MessageFlags, MessageHeader, MessageType, ZeroCopyPtr,
    MAX_INLINE_SIZE, MESSAGE_SIZE, PROTOCOL_VERSION,
};
