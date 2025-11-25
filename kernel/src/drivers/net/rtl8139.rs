//! Realtek RTL8139 Network Driver (wrapper)
//! 
//! Wraps C implementation of RTL8139 driver.

use super::NetworkDriver;

/// RTL8139 driver structure
pub struct Rtl8139Driver {
    base_addr: usize,
    mac: [u8; 6],
}

impl Rtl8139Driver {
    pub fn new(base_addr: usize) -> Self {
        Self {
            base_addr,
            mac: [0; 6],
        }
    }
    
    /// Detect RTL8139 cards via PCI
    pub fn detect() -> Option<Self> {
        // TODO: Scan PCI for device ID 0x10EC:0x8139 (RTL8139)
        None
    }
}

impl NetworkDriver for Rtl8139Driver {
    fn init(&mut self) -> Result<(), &'static str> {
        // TODO: Call C rtl8139_init()
        // Power on, software reset, read MAC
        Ok(())
    }
    
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        // TODO: Call C rtl8139_send()
        if data.len() > 1792 {
            return Err("Packet too large");
        }
        Ok(())
    }
    
    fn receive(&mut self) -> Option<&[u8]> {
        // TODO: Call C rtl8139_recv()
        None
    }
    
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
}
