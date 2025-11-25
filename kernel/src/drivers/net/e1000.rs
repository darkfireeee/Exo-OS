//! Intel E1000 Network Driver (wrapper)
//! 
//! Wraps C implementation of E1000 driver.

use super::NetworkDriver;

/// E1000 driver structure
pub struct E1000Driver {
    base_addr: usize,
    mac: [u8; 6],
}

impl E1000Driver {
    pub fn new(base_addr: usize) -> Self {
        Self {
            base_addr,
            mac: [0; 6],
        }
    }
    
    /// Detect E1000 cards via PCI
    pub fn detect() -> Option<Self> {
        // TODO: Scan PCI for device ID 0x8086:0x100E (E1000)
        None
    }
}

impl NetworkDriver for E1000Driver {
    fn init(&mut self) -> Result<(), &'static str> {
        // TODO: Call C e1000_init()
        // Read MAC address from EEPROM
        Ok(())
    }
    
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        // TODO: Call C e1000_send()
        if data.len() > 1518 {
            return Err("Packet too large");
        }
        Ok(())
    }
    
    fn receive(&mut self) -> Option<&[u8]> {
        // TODO: Call C e1000_recv()
        None
    }
    
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
}
