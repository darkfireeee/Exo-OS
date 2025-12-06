//! Network core layer

use super::{NetResult, NetError};

/// Network sockets
pub mod socket;

/// Network devices
pub mod device;

/// Network buffers
pub mod buffer;

/// Socket buffer (skb) - zero-copy packet management
pub mod skb;

/// Network device management
pub mod netdev;

/// Packet processing pipeline
pub mod packet;

/// Network interface abstraction
pub mod interface;

/// Statistics collection
pub mod stats;

// Re-exports
pub use skb::{SocketBuffer, SkbPool, SkbError};
pub use netdev::{NetworkDevice, DeviceManager, DeviceOps, DeviceType, DeviceState, DEVICE_MANAGER};
pub use packet::{PacketPipeline, PacketHook, PacketAction, PACKET_PIPELINE};
pub use interface::{NetworkInterface, InterfaceConfig, InterfaceManager, INTERFACE_MANAGER};
pub use stats::{NetworkStats, NetworkStatsSnapshot, NETWORK_STATS};

/// Initialize network core
pub fn init() -> NetResult<()> {
    log::debug!("Network core initialized");
    Ok(())
}
