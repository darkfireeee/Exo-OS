#![no_std]
#![doc = include_str!("../README.md")]

//! # exo_ipc - Communication Inter-Processus pour Exo-OS
//!
//! Bibliothèque IPC robuste et performante avec:
//! - Ring buffers lock-free (SPSC, MPSC)
//! - Messages typés avec versioning et checksums
//! - Zero-copy via mémoire partagée
//! - Handshake et flow control
//! - Sécurité capability-based
//!
//! ## Utilisation
//!
//! ### Canal SPSC (Single Producer Single Consumer)
//! ```ignore
//! use exo_ipc::channel;
//! use exo_ipc::types::{Message, MessageType};
//!
//! let (tx, rx) = channel::spsc(64)?;
//! let msg = Message::new(MessageType::Data);
//! tx.send(msg)?;
//! let received = rx.recv()?;
//! ```
//!
//! ### Canal MPSC (Multi Producer Single Consumer)
//! ```ignore
//! let (tx, rx) = channel::mpsc(64)?;
//! let tx2 = tx.clone(); // Plusieurs producteurs
//! ```
//!
//! ### Mémoire partagée (Zero-Copy)
//! ```ignore
//! use exo_ipc::shm::{SharedRegion, RegionPermissions};
//!
//! let region = SharedRegion::new(4096, RegionPermissions::READ_WRITE)?;
//! let mapping = region.map_readonly()?;
//! ```

extern crate alloc;

// Modules publics
pub mod types;
pub mod ring;
pub mod channel;
pub mod shm;
pub mod protocol;
pub mod util;

// Réexportations principales
pub use types::{
    Message, MessageType, MessageFlags, MessageHeader,
    IpcError, IpcResult, RecvError, SendError,
    Endpoint, EndpointId, EndpointType,
    Capability, CapabilityId, Permissions,
    MAX_INLINE_SIZE, MESSAGE_SIZE, PROTOCOL_VERSION,
};

pub use channel::{
    spsc, mpsc,
    SenderSpsc, ReceiverSpsc,
    SenderMpsc, ReceiverMpsc,
};

pub use shm::{
    SharedRegion, SharedMapping, MessagePool,
    RegionId, RegionPermissions,
};

pub use protocol::{
    HandshakeManager, SessionConfig, Capabilities,
    TokenBucketFlowController, CreditBasedFlowController,
};

pub use util::{
    AtomicStats, SequenceCounter,
    CachePadded, CACHE_LINE_SIZE,
    crc32c,
};

/// Version de la bibliothèque exo_ipc
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
