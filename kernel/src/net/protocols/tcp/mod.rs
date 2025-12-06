/// TCP Protocol Implementation
/// 
/// Complete TCP/IP protocol stack with:
/// - Socket API (connect, listen, accept, send, recv)
/// - TCP Fast Open (RFC 7413)
/// - Advanced features from kernel/src/net/tcp/*

pub mod socket;
pub mod listener;
pub mod fastopen;

pub use socket::{TcpSocket, TcpSocketError};
pub use listener::{TcpListener, ListenerState, ListenerError};
pub use fastopen::{TfoCookie, TfoManager, TfoStats};

// Re-export TCP core modules from kernel
pub use crate::net::tcp::{
    TcpConnection,
    TcpStateMachine,
    TcpState,
    TcpCongestion,
    TcpRetransmit,
    TcpSegment,
    TcpWindow,
    TcpOptions,
    TcpTimer,
};
