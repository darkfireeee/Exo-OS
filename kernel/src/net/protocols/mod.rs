/// Network Protocol Implementations
/// 
/// Organized protocol stack following clean architecture:
/// - TCP: Transmission Control Protocol ✅
/// - UDP: User Datagram Protocol ✅
/// - IP: Internet Protocol ✅
/// - Ethernet: Link layer ✅
/// - QUIC: Modern transport protocol ✅
/// - HTTP/2: Application protocol ✅
/// - TLS: Transport Layer Security ✅

pub mod tcp;
pub mod udp;
pub mod ip;
pub mod ethernet;
pub mod quic;
pub mod http2;
pub mod tls;

// Re-exports
pub use tcp::{TcpSocket, TcpListener, TcpSocketError, ListenerError};
pub use udp::{UdpSocket, UdpSocketError, UdpHeader, UdpDatagram};
pub use ip::{IgmpHeader, TunnelConfig, TunnelType, IcmpHeader, RoutingTable};
pub use ethernet::arp;
pub use quic::{QuicConnection, QuicClient, QuicError};
pub use http2::{Http2Connection, Http2Stream, Http2Frame};
pub use tls::{TlsConnection, TlsVersion, CipherSuite};
