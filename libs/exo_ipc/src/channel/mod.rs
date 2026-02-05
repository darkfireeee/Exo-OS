// libs/exo_ipc/src/channel/mod.rs
//! APIs de canaux IPC

pub mod bounded;

// Réexportations
pub use bounded::{
    spsc, mpsc,
    SenderSpsc, ReceiverSpsc,
    SenderMpsc, ReceiverMpsc,
    ChannelType,
};
