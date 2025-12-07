//! # Association Module
//! 
//! Association and reassociation handling

use alloc::vec::Vec;
use alloc::string::String;

/// Association state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssocState {
    Idle,
    Requesting,
    Associated,
    Failed,
}

/// Association context
pub struct AssocContext {
    pub state: AssocState,
    pub aid: Option<u16>,  // Association ID
    pub capabilities: u16,
    pub listen_interval: u16,
}

impl AssocContext {
    pub fn new() -> Self {
        Self {
            state: AssocState::Idle,
            aid: None,
            capabilities: 0x0431,  // ESS + Privacy + Short Preamble
            listen_interval: 10,
        }
    }
    
    /// Build association request
    pub fn build_request(
        &self,
        bssid: [u8; 6],
        ssid: &str,
        rates: &[u8],
        ht_caps: Option<&[u8]>,
        vht_caps: Option<&[u8]>,
        he_caps: Option<&[u8]>,
    ) -> Result<Vec<u8>, super::WiFiError> {
        let mut frame = Vec::new();
        
        // Capability Information
        frame.extend_from_slice(&self.capabilities.to_le_bytes());
        
        // Listen Interval
        frame.extend_from_slice(&self.listen_interval.to_le_bytes());
        
        // SSID IE
        frame.push(0);  // Element ID: SSID
        frame.push(ssid.len() as u8);
        frame.extend_from_slice(ssid.as_bytes());
        
        // Supported Rates IE
        frame.push(1);  // Element ID: Supported Rates
        let basic_rates = &rates[..rates.len().min(8)];
        frame.push(basic_rates.len() as u8);
        frame.extend_from_slice(basic_rates);
        
        // Extended Supported Rates IE (if needed)
        if rates.len() > 8 {
            frame.push(50);  // Element ID: Extended Supported Rates
            frame.push((rates.len() - 8) as u8);
            frame.extend_from_slice(&rates[8..]);
        }
        
        // HT Capabilities IE (802.11n)
        if let Some(caps) = ht_caps {
            frame.push(45);  // Element ID: HT Capabilities
            frame.push(caps.len() as u8);
            frame.extend_from_slice(caps);
        }
        
        // VHT Capabilities IE (802.11ac)
        if let Some(caps) = vht_caps {
            frame.push(191);  // Element ID: VHT Capabilities
            frame.push(caps.len() as u8);
            frame.extend_from_slice(caps);
        }
        
        // HE Capabilities IE (802.11ax / WiFi 6)
        if let Some(caps) = he_caps {
            frame.push(255);  // Element ID: Extension
            frame.push((caps.len() + 1) as u8);
            frame.push(35);   // Extension ID: HE Capabilities
            frame.extend_from_slice(caps);
        }
        
        Ok(frame)
    }
    
    /// Process association response
    pub fn process_response(
        &mut self,
        frame: &[u8],
    ) -> Result<(), super::WiFiError> {
        if frame.len() < 30 {
            return Err(super::WiFiError::InvalidFrame);
        }
        
        // Parse capability
        let _cap = u16::from_le_bytes([frame[24], frame[25]]);
        
        // Parse status code
        let status = u16::from_le_bytes([frame[26], frame[27]]);
        
        // Parse AID
        let aid = u16::from_le_bytes([frame[28], frame[29]]) & 0x3FFF;
        
        if status == 0 {
            self.state = AssocState::Associated;
            self.aid = Some(aid);
            Ok(())
        } else {
            self.state = AssocState::Failed;
            Err(super::WiFiError::AssociationFailed)
        }
    }
    
    /// Build reassociation request
    pub fn build_reassoc_request(
        &self,
        current_bssid: [u8; 6],
        new_bssid: [u8; 6],
        ssid: &str,
        rates: &[u8],
    ) -> Result<Vec<u8>, super::WiFiError> {
        let mut frame = Vec::new();
        
        // Capability Information
        frame.extend_from_slice(&self.capabilities.to_le_bytes());
        
        // Listen Interval
        frame.extend_from_slice(&self.listen_interval.to_le_bytes());
        
        // Current AP Address
        frame.extend_from_slice(&current_bssid);
        
        // SSID IE
        frame.push(0);
        frame.push(ssid.len() as u8);
        frame.extend_from_slice(ssid.as_bytes());
        
        // Supported Rates IE
        frame.push(1);
        frame.push(rates.len().min(8) as u8);
        frame.extend_from_slice(&rates[..rates.len().min(8)]);
        
        Ok(frame)
    }
}

/// Build HT Capabilities IE (802.11n)
pub fn build_ht_capabilities(streams: u8) -> Vec<u8> {
    let mut caps = Vec::new();
    
    // HT Capabilities Info (2 bytes)
    let mut ht_cap_info = 0u16;
    ht_cap_info |= 1 << 1;  // 40 MHz support
    ht_cap_info |= 1 << 5;  // Short GI for 20 MHz
    ht_cap_info |= 1 << 6;  // Short GI for 40 MHz
    caps.extend_from_slice(&ht_cap_info.to_le_bytes());
    
    // A-MPDU Parameters (1 byte)
    let ampdu_params = 0x17;  // Max length 64KB, Min MPDU spacing 8μs
    caps.push(ampdu_params);
    
    // Supported MCS Set (16 bytes)
    let mut mcs_set = vec![0u8; 16];
    // Enable MCS 0-7 for each stream
    for i in 0..streams {
        mcs_set[i as usize] = 0xFF;
    }
    caps.extend_from_slice(&mcs_set);
    
    // HT Extended Capabilities (2 bytes)
    caps.extend_from_slice(&[0x00, 0x00]);
    
    // Transmit Beamforming Capabilities (4 bytes)
    caps.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    
    // ASEL Capabilities (1 byte)
    caps.push(0x00);
    
    caps
}

/// Build VHT Capabilities IE (802.11ac)
pub fn build_vht_capabilities(streams: u8, width_160mhz: bool) -> Vec<u8> {
    let mut caps = Vec::new();
    
    // VHT Capabilities Info (4 bytes)
    let mut vht_cap_info = 0u32;
    vht_cap_info |= 2 << 2;   // Maximum MPDU length: 11454 bytes
    if width_160mhz {
        vht_cap_info |= 2 << 4;  // 160 MHz support
    }
    vht_cap_info |= 1 << 28;  // MU Beamformer
    vht_cap_info |= 1 << 29;  // MU Beamformee
    caps.extend_from_slice(&vht_cap_info.to_le_bytes());
    
    // VHT Supported MCS Set (8 bytes)
    let mut rx_mcs_map = 0u16;
    let mut tx_mcs_map = 0u16;
    
    for i in 0..streams {
        // MCS 0-9 support for each stream
        rx_mcs_map |= 2 << (i * 2);  // 2 = MCS 0-9
        tx_mcs_map |= 2 << (i * 2);
    }
    
    caps.extend_from_slice(&rx_mcs_map.to_le_bytes());
    caps.extend_from_slice(&[0x00, 0x00]);  // Rx highest rate
    caps.extend_from_slice(&tx_mcs_map.to_le_bytes());
    caps.extend_from_slice(&[0x00, 0x00]);  // Tx highest rate
    
    caps
}

/// Build HE Capabilities IE (802.11ax / WiFi 6)
pub fn build_he_capabilities(streams: u8, ofdma: bool) -> Vec<u8> {
    let mut caps = Vec::new();
    
    // MAC Capabilities Info (6 bytes)
    let mut mac_caps = [0u8; 6];
    if ofdma {
        mac_caps[0] |= 1 << 3;  // OFDMA RA support
    }
    caps.extend_from_slice(&mac_caps);
    
    // PHY Capabilities Info (11 bytes)
    let mut phy_caps = [0u8; 11];
    phy_caps[0] |= 1 << 0;  // 40 MHz in 2.4 GHz
    phy_caps[0] |= 1 << 1;  // 40/80 MHz in 5 GHz
    phy_caps[0] |= 1 << 2;  // 160 MHz in 5 GHz
    phy_caps[1] |= 1 << 7;  // SU Beamformer
    phy_caps[2] |= 1 << 0;  // SU Beamformee
    phy_caps[2] |= 1 << 3;  // MU Beamformer
    caps.extend_from_slice(&phy_caps);
    
    // Supported MCS and NSS Set (4 bytes per band)
    // 2.4 GHz and 5 GHz
    for _ in 0..2 {
        let mut mcs_nss = 0u16;
        for i in 0..streams {
            // MCS 0-11 support (1024-QAM)
            mcs_nss |= 2 << (i * 2);  // 2 = MCS 0-11
        }
        caps.extend_from_slice(&mcs_nss.to_le_bytes());
    }
    
    caps
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_assoc_context() {
        let ctx = AssocContext::new();
        assert_eq!(ctx.state, AssocState::Idle);
        assert!(ctx.aid.is_none());
    }
    
    #[test]
    fn test_ht_capabilities() {
        let caps = build_ht_capabilities(4);
        assert_eq!(caps.len(), 26);
    }
    
    #[test]
    fn test_vht_capabilities() {
        let caps = build_vht_capabilities(4, true);
        assert_eq!(caps.len(), 12);
    }
    
    #[test]
    fn test_he_capabilities() {
        let caps = build_he_capabilities(4, true);
        assert!(caps.len() > 0);
    }
}
