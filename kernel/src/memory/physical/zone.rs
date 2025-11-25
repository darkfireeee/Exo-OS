//! Memory zones (DMA, Normal, High)
//! 
//! Provides zone-based memory allocation

use crate::memory::PhysicalAddress;
use super::Frame;

/// Memory zone type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneType {
    /// DMA zone (0-16MB) for legacy DMA
    Dma,
    /// Normal zone (16MB-896MB)
    Normal,
    /// High memory zone (>896MB)
    High,
}

/// Memory zone
#[derive(Debug)]
pub struct Zone {
    /// Zone type
    pub zone_type: ZoneType,
    /// Starting physical address
    pub start: PhysicalAddress,
    /// Size in bytes
    pub size: usize,
    /// Free frames count
    pub free_frames: usize,
}

impl Zone {
    pub fn new(zone_type: ZoneType, start: PhysicalAddress, size: usize) -> Self {
        Self {
            zone_type,
            start,
            size,
            free_frames: size / super::FRAME_SIZE,
        }
    }
    
    /// Check if address is in this zone
    pub fn contains(&self, addr: PhysicalAddress) -> bool {
        let start = self.start.value();
        let end = start + self.size;
        let addr_val = addr.value();
        addr_val >= start && addr_val < end
    }
    
    /// Get zone for address
    pub fn zone_for_address(addr: PhysicalAddress) -> ZoneType {
        let addr_val = addr.value();
        if addr_val < 16 * 1024 * 1024 {
            ZoneType::Dma
        } else if addr_val < 896 * 1024 * 1024 {
            ZoneType::Normal
        } else {
            ZoneType::High
        }
    }
}

/// Zone allocator
pub struct ZoneAllocator {
    zones: [Option<Zone>; 3],
}

impl ZoneAllocator {
    pub fn new() -> Self {
        Self {
            zones: [None, None, None],
        }
    }
    
    /// Register a zone
    pub fn register_zone(&mut self, zone: Zone) {
        let index = match zone.zone_type {
            ZoneType::Dma => 0,
            ZoneType::Normal => 1,
            ZoneType::High => 2,
        };
        self.zones[index] = Some(zone);
    }
    
    /// Allocate frame from specific zone
    pub fn allocate_from_zone(&mut self, _zone_type: ZoneType) -> Option<Frame> {
        // TODO: Implement zone-specific allocation
        None
    }
}

impl Default for ZoneAllocator {
    fn default() -> Self {
        Self::new()
    }
}
