/// IGMP - Internet Group Management Protocol (RFC 3376)
/// 
/// Used for managing IPv4 multicast group memberships.
/// IGMPv3 adds support for source-specific multicast.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// IGMP Message Types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IgmpMessageType {
    /// Membership Query (0x11)
    MembershipQuery = 0x11,
    /// IGMPv1 Membership Report (0x12)
    V1MembershipReport = 0x12,
    /// IGMPv2 Membership Report (0x16)
    V2MembershipReport = 0x16,
    /// IGMPv2 Leave Group (0x17)
    LeaveGroup = 0x17,
    /// IGMPv3 Membership Report (0x22)
    V3MembershipReport = 0x22,
}

impl IgmpMessageType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x11 => Some(Self::MembershipQuery),
            0x12 => Some(Self::V1MembershipReport),
            0x16 => Some(Self::V2MembershipReport),
            0x17 => Some(Self::LeaveGroup),
            0x22 => Some(Self::V3MembershipReport),
            _ => None,
        }
    }
}

/// IGMP Header (IGMPv2)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IgmpHeader {
    /// Message type
    pub msg_type: u8,
    /// Max response time (in 1/10 second units)
    pub max_resp_time: u8,
    /// Checksum
    pub checksum: u16,
    /// Group address
    pub group_addr: [u8; 4],
}

impl IgmpHeader {
    /// Create a new IGMP header
    pub fn new(msg_type: IgmpMessageType, max_resp_time: u8, group_addr: [u8; 4]) -> Self {
        Self {
            msg_type: msg_type as u8,
            max_resp_time,
            checksum: 0,
            group_addr,
        }
    }

    /// Calculate checksum
    pub fn calculate_checksum(&self) -> u16 {
        let bytes = unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, 8) };
        let mut sum: u32 = 0;

        for chunk in bytes.chunks(2) {
            if chunk.len() == 2 {
                let word = u16::from_be_bytes([chunk[0], chunk[1]]);
                // Skip checksum field
                if chunk.as_ptr() as usize != &self.checksum as *const _ as usize {
                    sum += word as u32;
                }
            }
        }

        // Fold 32-bit sum to 16 bits
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }

        !sum as u16
    }

    /// Set checksum
    pub fn set_checksum(&mut self) {
        self.checksum = 0;
        self.checksum = self.calculate_checksum().to_be();
    }

    /// Verify checksum
    pub fn verify_checksum(&self) -> bool {
        let calculated = self.calculate_checksum();
        u16::from_be(self.checksum) == calculated
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> [u8; 8] {
        unsafe { core::mem::transmute(*self) }
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        Some(unsafe { core::ptr::read(data.as_ptr() as *const IgmpHeader) })
    }
}

/// IGMPv3 Group Record Type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GroupRecordType {
    /// Mode Is Include
    ModeIsInclude = 1,
    /// Mode Is Exclude
    ModeIsExclude = 2,
    /// Change To Include Mode
    ChangeToIncludeMode = 3,
    /// Change To Exclude Mode
    ChangeToExcludeMode = 4,
    /// Allow New Sources
    AllowNewSources = 5,
    /// Block Old Sources
    BlockOldSources = 6,
}

/// IGMPv3 Group Record
#[derive(Debug, Clone)]
pub struct GroupRecord {
    /// Record type
    pub record_type: GroupRecordType,
    /// Multicast address
    pub multicast_addr: [u8; 4],
    /// Source addresses
    pub sources: Vec<[u8; 4]>,
    /// Auxiliary data
    pub aux_data: Vec<u8>,
}

impl GroupRecord {
    /// Create a new group record
    pub fn new(record_type: GroupRecordType, multicast_addr: [u8; 4]) -> Self {
        Self {
            record_type,
            multicast_addr,
            sources: Vec::new(),
            aux_data: Vec::new(),
        }
    }

    /// Add a source address
    pub fn add_source(&mut self, source: [u8; 4]) {
        self.sources.push(source);
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Record type
        bytes.push(self.record_type as u8);
        // Aux data length (in 32-bit words)
        bytes.push((self.aux_data.len() / 4) as u8);
        // Number of sources
        bytes.extend_from_slice(&(self.sources.len() as u16).to_be_bytes());
        // Multicast address
        bytes.extend_from_slice(&self.multicast_addr);
        // Source addresses
        for source in &self.sources {
            bytes.extend_from_slice(source);
        }
        // Auxiliary data
        bytes.extend_from_slice(&self.aux_data);

        bytes
    }
}

/// IGMPv3 Membership Report
#[derive(Debug, Clone)]
pub struct IgmpV3Report {
    /// Group records
    pub records: Vec<GroupRecord>,
}

impl IgmpV3Report {
    /// Create a new IGMPv3 report
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Add a group record
    pub fn add_record(&mut self, record: GroupRecord) {
        self.records.push(record);
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Type (0x22)
        bytes.push(0x22);
        // Reserved
        bytes.push(0);
        // Checksum (will be calculated later)
        bytes.extend_from_slice(&[0, 0]);
        // Reserved
        bytes.extend_from_slice(&[0, 0]);
        // Number of group records
        bytes.extend_from_slice(&(self.records.len() as u16).to_be_bytes());

        // Group records
        for record in &self.records {
            bytes.extend_from_slice(&record.to_bytes());
        }

        // Calculate and set checksum
        let checksum = Self::calculate_checksum(&bytes);
        bytes[2] = (checksum >> 8) as u8;
        bytes[3] = (checksum & 0xff) as u8;

        bytes
    }

    /// Calculate checksum
    fn calculate_checksum(data: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        for chunk in data.chunks(2) {
            if chunk.len() == 2 {
                let word = u16::from_be_bytes([chunk[0], chunk[1]]);
                // Skip checksum field (bytes 2-3)
                if chunk.as_ptr() as usize - data.as_ptr() as usize != 2 {
                    sum += word as u32;
                }
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }

        // Fold 32-bit sum to 16 bits
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }

        !sum as u16
    }
}

/// IGMP statistics
#[derive(Debug)]
pub struct IgmpStats {
    /// Queries received
    pub queries_received: AtomicU64,
    /// Queries sent
    pub queries_sent: AtomicU64,
    /// Reports sent
    pub reports_sent: AtomicU64,
    /// Reports received
    pub reports_received: AtomicU64,
    /// Leaves sent
    pub leaves_sent: AtomicU64,
    /// Leaves received
    pub leaves_received: AtomicU64,
    /// Checksum errors
    pub checksum_errors: AtomicU64,
}

impl IgmpStats {
    pub fn new() -> Self {
        Self {
            queries_received: AtomicU64::new(0),
            queries_sent: AtomicU64::new(0),
            reports_sent: AtomicU64::new(0),
            reports_received: AtomicU64::new(0),
            leaves_sent: AtomicU64::new(0),
            leaves_received: AtomicU64::new(0),
            checksum_errors: AtomicU64::new(0),
        }
    }
}

/// Global IGMP statistics
static IGMP_STATS: Mutex<IgmpStats> = Mutex::new(IgmpStats {
    queries_received: AtomicU64::new(0),
    queries_sent: AtomicU64::new(0),
    reports_sent: AtomicU64::new(0),
    reports_received: AtomicU64::new(0),
    leaves_sent: AtomicU64::new(0),
    leaves_received: AtomicU64::new(0),
    checksum_errors: AtomicU64::new(0),
});

/// Get IGMP statistics
pub fn get_stats() -> &'static IgmpStats {
    unsafe { &*(IGMP_STATS.lock().as_ref() as *const IgmpStats) }
}
