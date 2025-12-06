//! # VLAN (Virtual LAN) Support
//! 
//! IEEE 802.1Q VLAN tagging

use crate::net::ethernet::MacAddress;

/// VLAN ID (12 bits, 0-4095)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VlanId(u16);

impl VlanId {
    pub const MIN: u16 = 1;
    pub const MAX: u16 = 4094;
    pub const RESERVED_0: u16 = 0;
    pub const RESERVED_4095: u16 = 4095;
    
    pub fn new(id: u16) -> Result<Self, VlanError> {
        if id == 0 || id == 4095 {
            return Err(VlanError::ReservedId);
        }
        if id > Self::MAX {
            return Err(VlanError::InvalidId);
        }
        Ok(Self(id))
    }
    
    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Priority Code Point (PCP) - 3 bits
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VlanPriority {
    BestEffort = 0,       // Background
    Background = 1,       // Best effort (default)
    ExcellentEffort = 2,  // Excellent effort
    CriticalApps = 3,     // Critical applications
    Video = 4,            // Video, < 100ms latency
    Voice = 5,            // Voice, < 10ms latency
    InternetworkControl = 6, // Internetwork control
    NetworkControl = 7,   // Network control
}

impl From<u8> for VlanPriority {
    fn from(val: u8) -> Self {
        match val & 0x7 {
            0 => Self::BestEffort,
            1 => Self::Background,
            2 => Self::ExcellentEffort,
            3 => Self::CriticalApps,
            4 => Self::Video,
            5 => Self::Voice,
            6 => Self::InternetworkControl,
            7 => Self::NetworkControl,
            _ => Self::BestEffort,
        }
    }
}

/// VLAN Tag (802.1Q)
#[derive(Debug, Clone, Copy)]
pub struct VlanTag {
    /// Priority Code Point (3 bits)
    pub pcp: VlanPriority,
    
    /// Drop Eligible Indicator (1 bit)
    pub dei: bool,
    
    /// VLAN ID (12 bits)
    pub vlan_id: VlanId,
}

impl VlanTag {
    /// Parse depuis TCI (Tag Control Information - 16 bits)
    pub fn from_tci(tci: u16) -> Result<Self, VlanError> {
        let pcp = VlanPriority::from((tci >> 13) as u8);
        let dei = (tci & 0x1000) != 0;
        let vlan_id = VlanId::new(tci & 0x0FFF)?;
        
        Ok(Self { pcp, dei, vlan_id })
    }
    
    /// Encode vers TCI
    pub fn to_tci(&self) -> u16 {
        let pcp_bits = (self.pcp as u16) << 13;
        let dei_bit = if self.dei { 0x1000 } else { 0 };
        let vlan_bits = self.vlan_id.value();
        
        pcp_bits | dei_bit | vlan_bits
    }
}

/// VLAN Frame (802.1Q tagged)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct VlanFrame {
    /// Destination MAC
    pub dst_mac: MacAddress,
    
    /// Source MAC
    pub src_mac: MacAddress,
    
    /// TPID (Tag Protocol Identifier) = 0x8100
    pub tpid: u16,
    
    /// TCI (Tag Control Information)
    pub tci: u16,
    
    /// EtherType (protocole encapsulé)
    pub ethertype: u16,
}

impl VlanFrame {
    pub const TPID: u16 = 0x8100;
    pub const SIZE: usize = 18; // 6 + 6 + 2 + 2 + 2
    
    pub fn new(
        dst_mac: MacAddress,
        src_mac: MacAddress,
        tag: VlanTag,
        ethertype: u16,
    ) -> Self {
        Self {
            dst_mac,
            src_mac,
            tpid: Self::TPID.to_be(),
            tci: tag.to_tci().to_be(),
            ethertype: ethertype.to_be(),
        }
    }
    
    /// Parse depuis bytes
    pub fn parse(data: &[u8]) -> Result<Self, VlanError> {
        if data.len() < Self::SIZE {
            return Err(VlanError::TooShort);
        }
        
        let mut dst_mac = [0u8; 6];
        dst_mac.copy_from_slice(&data[0..6]);
        
        let mut src_mac = [0u8; 6];
        src_mac.copy_from_slice(&data[6..12]);
        
        let tpid = u16::from_be_bytes([data[12], data[13]]);
        if tpid != Self::TPID {
            return Err(VlanError::InvalidTpid);
        }
        
        let tci = u16::from_be_bytes([data[14], data[15]]);
        let ethertype = u16::from_be_bytes([data[16], data[17]]);
        
        Ok(Self {
            dst_mac: MacAddress::new(dst_mac),
            src_mac: MacAddress::new(src_mac),
            tpid: tpid.to_be(),
            tci: tci.to_be(),
            ethertype: ethertype.to_be(),
        })
    }
    
    /// Obtient le tag VLAN
    pub fn tag(&self) -> Result<VlanTag, VlanError> {
        VlanTag::from_tci(u16::from_be(self.tci))
    }
    
    /// Obtient l'EtherType
    pub fn ethertype(&self) -> u16 {
        u16::from_be(self.ethertype)
    }
}

/// Q-in-Q (802.1ad) - Double VLAN tagging
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct QinQFrame {
    pub dst_mac: MacAddress,
    pub src_mac: MacAddress,
    
    /// Outer tag (S-TAG, Service TAG)
    pub outer_tpid: u16, // 0x88A8
    pub outer_tci: u16,
    
    /// Inner tag (C-TAG, Customer TAG)
    pub inner_tpid: u16, // 0x8100
    pub inner_tci: u16,
    
    pub ethertype: u16,
}

impl QinQFrame {
    pub const OUTER_TPID: u16 = 0x88A8; // 802.1ad
    pub const INNER_TPID: u16 = 0x8100; // 802.1Q
    pub const SIZE: usize = 22; // 6 + 6 + 2 + 2 + 2 + 2 + 2
    
    pub fn new(
        dst_mac: MacAddress,
        src_mac: MacAddress,
        outer_tag: VlanTag,
        inner_tag: VlanTag,
        ethertype: u16,
    ) -> Self {
        Self {
            dst_mac,
            src_mac,
            outer_tpid: Self::OUTER_TPID.to_be(),
            outer_tci: outer_tag.to_tci().to_be(),
            inner_tpid: Self::INNER_TPID.to_be(),
            inner_tci: inner_tag.to_tci().to_be(),
            ethertype: ethertype.to_be(),
        }
    }
}

/// VLAN Errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VlanError {
    InvalidId,
    ReservedId,
    TooShort,
    InvalidTpid,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_vlan_id() {
        assert!(VlanId::new(0).is_err()); // Reserved
        assert!(VlanId::new(4095).is_err()); // Reserved
        assert!(VlanId::new(100).is_ok());
        
        let id = VlanId::new(100).unwrap();
        assert_eq!(id.value(), 100);
    }
    
    #[test]
    fn test_vlan_tag() {
        let tag = VlanTag {
            pcp: VlanPriority::Voice,
            dei: false,
            vlan_id: VlanId::new(100).unwrap(),
        };
        
        let tci = tag.to_tci();
        assert_eq!(tci & 0x0FFF, 100); // VLAN ID
        assert_eq!((tci >> 13), 5); // PCP (Voice = 5)
        
        let parsed = VlanTag::from_tci(tci).unwrap();
        assert_eq!(parsed.vlan_id.value(), 100);
    }
    
    #[test]
    fn test_vlan_frame() {
        let dst = MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let src = MacAddress::new([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        
        let tag = VlanTag {
            pcp: VlanPriority::Video,
            dei: false,
            vlan_id: VlanId::new(200).unwrap(),
        };
        
        let frame = VlanFrame::new(dst, src, tag, 0x0800);
        
        assert_eq!(u16::from_be(frame.tpid), 0x8100);
        assert_eq!(u16::from_be(frame.ethertype), 0x0800);
        
        let parsed_tag = frame.tag().unwrap();
        assert_eq!(parsed_tag.vlan_id.value(), 200);
    }
}
