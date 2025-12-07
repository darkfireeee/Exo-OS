//! Loopback Device (lo) - Virtual Local Interface
//!
//! Perfect loopback implementation for local testing.
//! Always delivers packets instantly with zero loss.

use super::{NetworkDevice, DeviceCapabilities, DeviceStats, DriverError, DriverResult};
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use crate::sync::SpinLock;

/// Loopback device
pub struct LoopbackDevice {
    capabilities: DeviceCapabilities,
    stats: SpinLock<DeviceStats>,
    rx_queue: SpinLock<VecDeque<Vec<u8>>>,
}

impl LoopbackDevice {
    pub fn new() -> Self {
        let mut caps = DeviceCapabilities::default();
        caps.mac_address = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        caps.max_mtu = 65536; // No real MTU limit for loopback
        caps.checksum_offload = true; // No need for checksums
        
        Self {
            capabilities: caps,
            stats: SpinLock::new(DeviceStats::default()),
            rx_queue: SpinLock::new(VecDeque::with_capacity(128)),
        }
    }
}

impl NetworkDevice for LoopbackDevice {
    fn name(&self) -> &str {
        "lo"
    }
    
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.capabilities
    }
    
    fn stats(&self) -> DeviceStats {
        *self.stats.lock()
    }
    
    fn send(&self, packet: &[u8]) -> Result<(), DriverError> {
        // Loopback: send is receive
        let mut queue = self.rx_queue.lock();
        let mut stats = self.stats.lock();
        
        if queue.len() >= 128 {
            stats.tx_dropped += 1;
            return Err(DriverError::QueueFull);
        }
        
        queue.push_back(packet.to_vec());
        stats.tx_packets += 1;
        stats.tx_bytes += packet.len() as u64;
        stats.rx_packets += 1;
        stats.rx_bytes += packet.len() as u64;
        
        Ok(())
    }
    
    fn receive(&self) -> Result<Vec<Vec<u8>>, DriverError> {
        let mut queue = self.rx_queue.lock();
        let mut packets = Vec::new();
        
        while let Some(packet) = queue.pop_front() {
            packets.push(packet);
        }
        
        Ok(packets)
    }
    
    fn set_promiscuous(&self, _enabled: bool) -> Result<(), DriverError> {
        // Loopback is always "promiscuous" to itself
        Ok(())
    }
    
    fn set_mac_address(&self, _mac: [u8; 6]) -> Result<(), DriverError> {
        // Loopback doesn't use MAC addresses
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_loopback_send_receive() {
        let dev = LoopbackDevice::new();
        let data = b"Hello, loopback!";
        
        // Send
        dev.send(data).unwrap();
        
        // Receive
        let packets = dev.receive().unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].as_slice(), data);
        
        // Stats
        let stats = dev.stats();
        assert_eq!(stats.tx_packets, 1);
        assert_eq!(stats.rx_packets, 1);
        assert_eq!(stats.tx_bytes, data.len() as u64);
    }
}
