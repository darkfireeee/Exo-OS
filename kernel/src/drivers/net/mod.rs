//! Network Drivers Module
//! 
//! Wrappers for common network cards using C drivers.

pub mod e1000;
pub mod rtl8139;
pub mod virtio_net;

pub use e1000::E1000Driver;
pub use rtl8139::Rtl8139Driver;
pub use virtio_net::VirtioNetDriver;

/// Network driver trait
pub trait NetworkDriver {
    /// Initialize the driver
    fn init(&mut self) -> Result<(), &'static str>;
    
    /// Send packet
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str>;
    
    /// Receive packet (non-blocking)
    fn receive(&mut self) -> Option<&[u8]>;
    
    /// Get MAC address
    fn mac_address(&self) -> [u8; 6];
}
