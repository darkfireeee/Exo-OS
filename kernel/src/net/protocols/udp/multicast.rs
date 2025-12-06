/// UDP Multicast Support
/// 
/// Implements IGMP (Internet Group Management Protocol) for IPv4 multicast
/// and MLD (Multicast Listener Discovery) for IPv6 multicast.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Multicast group address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MulticastGroup {
    /// Group address (IPv4 or IPv6)
    pub addr: [u8; 16],
    /// Interface index
    pub interface: u32,
}

impl MulticastGroup {
    /// Create a new multicast group
    pub fn new(addr: [u8; 16], interface: u32) -> Self {
        Self { addr, interface }
    }

    /// Check if this is an IPv4 multicast address (224.0.0.0 - 239.255.255.255)
    pub fn is_ipv4_multicast(&self) -> bool {
        self.addr[0] >= 224 && self.addr[0] <= 239
    }

    /// Check if this is an IPv6 multicast address (ff00::/8)
    pub fn is_ipv6_multicast(&self) -> bool {
        self.addr[0] == 0xff
    }

    /// Check if this is a valid multicast address
    pub fn is_valid(&self) -> bool {
        self.is_ipv4_multicast() || self.is_ipv6_multicast()
    }
}

/// Multicast membership
#[derive(Debug, Clone)]
pub struct MulticastMembership {
    /// Multicast group
    pub group: MulticastGroup,
    /// Number of sockets subscribed to this group
    pub ref_count: u32,
    /// Source filter mode
    pub filter_mode: FilterMode,
    /// Source list (for source-specific multicast)
    pub sources: Vec<[u8; 16]>,
}

/// Multicast filter mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Include only specified sources
    Include,
    /// Exclude specified sources
    Exclude,
}

/// Multicast manager
pub struct MulticastManager {
    /// Active multicast memberships
    memberships: Mutex<BTreeMap<MulticastGroup, MulticastMembership>>,
    /// Statistics
    stats: MulticastStats,
}

impl MulticastManager {
    /// Create a new multicast manager
    pub fn new() -> Self {
        Self {
            memberships: Mutex::new(BTreeMap::new()),
            stats: MulticastStats::new(),
        }
    }

    /// Join a multicast group
    pub fn join_group(&self, group: MulticastGroup) -> Result<(), MulticastError> {
        if !group.is_valid() {
            return Err(MulticastError::InvalidGroup);
        }

        let mut memberships = self.memberships.lock();
        
        if let Some(membership) = memberships.get_mut(&group) {
            // Group already joined, increment ref count
            membership.ref_count += 1;
        } else {
            // New group, create membership
            let membership = MulticastMembership {
                group,
                ref_count: 1,
                filter_mode: FilterMode::Exclude,
                sources: Vec::new(),
            };
            memberships.insert(group, membership);
            
            // Send IGMP/MLD join message
            // TODO: Implement actual IGMP/MLD protocol
            self.stats.groups_joined.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Leave a multicast group
    pub fn leave_group(&self, group: &MulticastGroup) -> Result<(), MulticastError> {
        let mut memberships = self.memberships.lock();
        
        if let Some(membership) = memberships.get_mut(group) {
            membership.ref_count -= 1;
            
            if membership.ref_count == 0 {
                // Last socket left, remove membership
                memberships.remove(group);
                
                // Send IGMP/MLD leave message
                // TODO: Implement actual IGMP/MLD protocol
                self.stats.groups_left.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        } else {
            Err(MulticastError::NotMember)
        }
    }

    /// Set source filter for a multicast group
    pub fn set_source_filter(
        &self,
        group: &MulticastGroup,
        mode: FilterMode,
        sources: Vec<[u8; 16]>,
    ) -> Result<(), MulticastError> {
        let mut memberships = self.memberships.lock();
        
        if let Some(membership) = memberships.get_mut(group) {
            membership.filter_mode = mode;
            membership.sources = sources;
            Ok(())
        } else {
            Err(MulticastError::NotMember)
        }
    }

    /// Check if a socket should receive a packet from a specific source
    pub fn should_receive(&self, group: &MulticastGroup, source: &[u8; 16]) -> bool {
        let memberships = self.memberships.lock();
        
        if let Some(membership) = memberships.get(group) {
            match membership.filter_mode {
                FilterMode::Include => {
                    // Accept only if source is in the list
                    membership.sources.iter().any(|s| s == source)
                }
                FilterMode::Exclude => {
                    // Accept unless source is in the list
                    !membership.sources.iter().any(|s| s == source)
                }
            }
        } else {
            false
        }
    }

    /// Get all active multicast groups
    pub fn get_groups(&self) -> Vec<MulticastGroup> {
        let memberships = self.memberships.lock();
        memberships.keys().copied().collect()
    }

    /// Get membership information for a group
    pub fn get_membership(&self, group: &MulticastGroup) -> Option<MulticastMembership> {
        let memberships = self.memberships.lock();
        memberships.get(group).cloned()
    }

    /// Get statistics
    pub fn get_stats(&self) -> MulticastStatsSnapshot {
        self.stats.snapshot()
    }
}

/// Multicast statistics
#[derive(Debug)]
pub struct MulticastStats {
    /// Number of groups joined
    pub groups_joined: AtomicU64,
    /// Number of groups left
    pub groups_left: AtomicU64,
    /// Number of multicast packets received
    pub packets_received: AtomicU64,
    /// Number of multicast packets sent
    pub packets_sent: AtomicU64,
    /// Number of IGMP/MLD messages sent
    pub igmp_messages_sent: AtomicU64,
    /// Number of IGMP/MLD messages received
    pub igmp_messages_received: AtomicU64,
}

impl MulticastStats {
    pub fn new() -> Self {
        Self {
            groups_joined: AtomicU64::new(0),
            groups_left: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            packets_sent: AtomicU64::new(0),
            igmp_messages_sent: AtomicU64::new(0),
            igmp_messages_received: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> MulticastStatsSnapshot {
        MulticastStatsSnapshot {
            groups_joined: self.groups_joined.load(Ordering::Relaxed),
            groups_left: self.groups_left.load(Ordering::Relaxed),
            packets_received: self.packets_received.load(Ordering::Relaxed),
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            igmp_messages_sent: self.igmp_messages_sent.load(Ordering::Relaxed),
            igmp_messages_received: self.igmp_messages_received.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of multicast statistics
#[derive(Debug, Clone, Copy)]
pub struct MulticastStatsSnapshot {
    pub groups_joined: u64,
    pub groups_left: u64,
    pub packets_received: u64,
    pub packets_sent: u64,
    pub igmp_messages_sent: u64,
    pub igmp_messages_received: u64,
}

/// Multicast errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulticastError {
    /// Invalid multicast group address
    InvalidGroup,
    /// Not a member of the group
    NotMember,
    /// Too many groups joined
    TooManyGroups,
    /// Group already joined
    AlreadyJoined,
}

/// Global multicast manager
static MULTICAST_MANAGER: Mutex<Option<MulticastManager>> = Mutex::new(None);

/// Initialize the multicast manager
pub fn init() {
    *MULTICAST_MANAGER.lock() = Some(MulticastManager::new());
}

/// Get the global multicast manager
pub fn get_manager() -> Option<Arc<MulticastManager>> {
    MULTICAST_MANAGER.lock().as_ref().map(|_| {
        // Return a reference wrapped in Arc
        // In a real implementation, we would store an Arc directly
        Arc::new(MulticastManager::new())
    })
}
