//! # Regulatory Domain Management
//! 
//! Manages:
//! - Country-specific channel regulations
//! - Transmit power limits
//! - DFS (Dynamic Frequency Selection)
//! - Passive/active scanning rules

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::sync::SpinLock;

/// Regulatory domain manager
pub struct RegulatoryManager {
    current_country: SpinLock<[u8; 2]>,
    domains: SpinLock<BTreeMap<[u8; 2], RegulatoryDomain>>,
}

impl RegulatoryManager {
    pub fn new() -> Self {
        let mut manager = Self {
            current_country: SpinLock::new(*b"US"),
            domains: SpinLock::new(BTreeMap::new()),
        };
        
        manager.init_domains();
        manager
    }
    
    /// Initialize known regulatory domains
    fn init_domains(&mut self) {
        let mut domains = self.domains.lock();
        
        // United States (FCC)
        domains.insert(*b"US", RegulatoryDomain {
            country: *b"US",
            dfs_region: DfsRegion::Fcc,
            channels_2_4ghz: vec![
                ChannelRule { channel: 1, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 2, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 3, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 4, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 5, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 6, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 7, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 8, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 9, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 10, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 11, max_power: 30, flags: ChannelFlags::empty() },
            ],
            channels_5ghz: vec![
                // UNII-1 (Lower band)
                ChannelRule { channel: 36, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 40, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 44, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 48, max_power: 23, flags: ChannelFlags::empty() },
                
                // UNII-2A (DFS required)
                ChannelRule { channel: 52, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 56, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 60, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 64, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                
                // UNII-2C (DFS required)
                ChannelRule { channel: 100, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 104, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 108, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 112, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 116, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 120, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 124, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 128, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 132, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 136, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 140, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                ChannelRule { channel: 144, max_power: 24, flags: ChannelFlags::DFS | ChannelFlags::NO_IBSS },
                
                // UNII-3 (Upper band)
                ChannelRule { channel: 149, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 153, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 157, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 161, max_power: 30, flags: ChannelFlags::empty() },
                ChannelRule { channel: 165, max_power: 30, flags: ChannelFlags::empty() },
            ],
        });
        
        // European Union (ETSI)
        domains.insert(*b"EU", RegulatoryDomain {
            country: *b"EU",
            dfs_region: DfsRegion::Etsi,
            channels_2_4ghz: vec![
                ChannelRule { channel: 1, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 2, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 3, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 4, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 5, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 6, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 7, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 8, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 9, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 10, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 11, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 12, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 13, max_power: 20, flags: ChannelFlags::empty() },
            ],
            channels_5ghz: vec![
                ChannelRule { channel: 36, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 40, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 44, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 48, max_power: 23, flags: ChannelFlags::empty() },
                ChannelRule { channel: 52, max_power: 23, flags: ChannelFlags::DFS },
                ChannelRule { channel: 56, max_power: 23, flags: ChannelFlags::DFS },
                ChannelRule { channel: 60, max_power: 23, flags: ChannelFlags::DFS },
                ChannelRule { channel: 64, max_power: 23, flags: ChannelFlags::DFS },
                ChannelRule { channel: 100, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 104, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 108, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 112, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 116, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 120, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 124, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 128, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 132, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 136, max_power: 30, flags: ChannelFlags::DFS },
                ChannelRule { channel: 140, max_power: 30, flags: ChannelFlags::DFS },
            ],
        });
        
        // Japan (MIC)
        domains.insert(*b"JP", RegulatoryDomain {
            country: *b"JP",
            dfs_region: DfsRegion::Japan,
            channels_2_4ghz: vec![
                ChannelRule { channel: 1, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 2, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 3, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 4, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 5, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 6, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 7, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 8, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 9, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 10, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 11, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 12, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 13, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 14, max_power: 20, flags: ChannelFlags::NO_OFDM },
            ],
            channels_5ghz: vec![
                ChannelRule { channel: 36, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 40, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 44, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 48, max_power: 20, flags: ChannelFlags::empty() },
                ChannelRule { channel: 52, max_power: 20, flags: ChannelFlags::DFS },
                ChannelRule { channel: 56, max_power: 20, flags: ChannelFlags::DFS },
                ChannelRule { channel: 60, max_power: 20, flags: ChannelFlags::DFS },
                ChannelRule { channel: 64, max_power: 20, flags: ChannelFlags::DFS },
            ],
        });
    }
    
    /// Set current country
    pub fn set_country(&self, country: [u8; 2]) -> Result<(), super::WiFiError> {
        let domains = self.domains.lock();
        if !domains.contains_key(&country) {
            return Err(super::WiFiError::InvalidChannel);
        }
        
        *self.current_country.lock() = country;
        Ok(())
    }
    
    /// Get current country
    pub fn get_country(&self) -> [u8; 2] {
        *self.current_country.lock()
    }
    
    /// Check if channel is allowed
    pub fn is_channel_allowed(&self, channel: u8) -> bool {
        let country = *self.current_country.lock();
        let domains = self.domains.lock();
        
        if let Some(domain) = domains.get(&country) {
            domain.is_channel_allowed(channel)
        } else {
            false
        }
    }
    
    /// Get max power for channel
    pub fn get_max_power(&self, channel: u8) -> Option<i8> {
        let country = *self.current_country.lock();
        let domains = self.domains.lock();
        
        domains.get(&country)?.get_max_power(channel)
    }
    
    /// Check if channel requires DFS
    pub fn requires_dfs(&self, channel: u8) -> bool {
        let country = *self.current_country.lock();
        let domains = self.domains.lock();
        
        if let Some(domain) = domains.get(&country) {
            domain.requires_dfs(channel)
        } else {
            false
        }
    }
    
    /// Get allowed channels
    pub fn get_allowed_channels(&self) -> Vec<u8> {
        let country = *self.current_country.lock();
        let domains = self.domains.lock();
        
        if let Some(domain) = domains.get(&country) {
            domain.get_allowed_channels()
        } else {
            Vec::new()
        }
    }
}

/// Regulatory domain
#[derive(Clone)]
pub struct RegulatoryDomain {
    pub country: [u8; 2],
    pub dfs_region: DfsRegion,
    pub channels_2_4ghz: Vec<ChannelRule>,
    pub channels_5ghz: Vec<ChannelRule>,
}

impl RegulatoryDomain {
    pub fn is_channel_allowed(&self, channel: u8) -> bool {
        self.channels_2_4ghz.iter().any(|r| r.channel == channel) ||
        self.channels_5ghz.iter().any(|r| r.channel == channel)
    }
    
    pub fn get_max_power(&self, channel: u8) -> Option<i8> {
        self.channels_2_4ghz.iter()
            .chain(self.channels_5ghz.iter())
            .find(|r| r.channel == channel)
            .map(|r| r.max_power)
    }
    
    pub fn requires_dfs(&self, channel: u8) -> bool {
        self.channels_5ghz.iter()
            .find(|r| r.channel == channel)
            .map(|r| r.flags.contains(ChannelFlags::DFS))
            .unwrap_or(false)
    }
    
    pub fn get_allowed_channels(&self) -> Vec<u8> {
        let mut channels = Vec::new();
        
        for rule in &self.channels_2_4ghz {
            channels.push(rule.channel);
        }
        
        for rule in &self.channels_5ghz {
            channels.push(rule.channel);
        }
        
        channels
    }
}

/// Channel rule
#[derive(Clone)]
pub struct ChannelRule {
    pub channel: u8,
    pub max_power: i8,  // dBm
    pub flags: ChannelFlags,
}

/// Channel flags
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelFlags(u8);

impl ChannelFlags {
    pub const DISABLED: Self = Self(1 << 0);
    pub const PASSIVE_SCAN: Self = Self(1 << 1);
    pub const NO_IBSS: Self = Self(1 << 2);
    pub const DFS: Self = Self(1 << 3);
    pub const NO_OFDM: Self = Self(1 << 4);
    
    pub const fn empty() -> Self {
        Self(0)
    }
    
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl core::ops::BitOr for ChannelFlags {
    type Output = Self;
    
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// DFS regions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfsRegion {
    Unset,
    Fcc,      // USA (FCC)
    Etsi,     // Europe (ETSI)
    Japan,    // Japan (MIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_regulatory_manager() {
        let reg = RegulatoryManager::new();
        assert_eq!(reg.get_country(), *b"US");
        assert!(reg.is_channel_allowed(6));
        assert!(reg.is_channel_allowed(36));
    }
    
    #[test]
    fn test_dfs_channels() {
        let reg = RegulatoryManager::new();
        assert!(!reg.requires_dfs(36));  // UNII-1, no DFS
        assert!(reg.requires_dfs(52));   // UNII-2A, DFS required
    }
    
    #[test]
    fn test_power_limits() {
        let reg = RegulatoryManager::new();
        assert_eq!(reg.get_max_power(6), Some(30));   // 2.4 GHz
        assert_eq!(reg.get_max_power(149), Some(30)); // 5 GHz UNII-3
    }
}
