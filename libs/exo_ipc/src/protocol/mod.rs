// libs/exo_ipc/src/protocol/mod.rs
//! Protocoles IPC: handshake, flow control, etc.

pub mod handshake;
pub mod flow_control;

// Réexportations
pub use handshake::{
    HandshakeManager, HandshakeState, SessionConfig, Capabilities,
};
pub use flow_control::{
    FlowStrategy, TokenBucketFlowController, SlidingWindowFlowController,
    CreditBasedFlowController,
};
