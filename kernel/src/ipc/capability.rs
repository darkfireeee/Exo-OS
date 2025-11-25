//! IPC Capabilities - Fine-grained access control for IPC operations
//!
//! Capabilities system for securing IPC channels and shared memory

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Capability ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(pub u64);

/// IPC capability types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityType {
    /// Can send messages to channel
    Send,
    /// Can receive messages from channel
    Receive,
    /// Can create new channels
    Create,
    /// Can destroy channels
    Destroy,
    /// Can map shared memory
    MapMemory,
    /// Can unmap shared memory
    UnmapMemory,
    /// Can grant capabilities to others
    Grant,
    /// Full access (all permissions)
    Admin,
}

/// Capability permission flags
#[derive(Debug, Clone, Copy)]
pub struct CapabilityFlags {
    pub can_send: bool,
    pub can_receive: bool,
    pub can_create: bool,
    pub can_destroy: bool,
    pub can_map: bool,
    pub can_unmap: bool,
    pub can_grant: bool,
    pub is_admin: bool,
}

impl CapabilityFlags {
    /// No permissions
    pub const NONE: Self = Self {
        can_send: false,
        can_receive: false,
        can_create: false,
        can_destroy: false,
        can_map: false,
        can_unmap: false,
        can_grant: false,
        is_admin: false,
    };
    
    /// Read-only (receive)
    pub const READ_ONLY: Self = Self {
        can_send: false,
        can_receive: true,
        can_create: false,
        can_destroy: false,
        can_map: false,
        can_unmap: false,
        can_grant: false,
        is_admin: false,
    };
    
    /// Write-only (send)
    pub const WRITE_ONLY: Self = Self {
        can_send: true,
        can_receive: false,
        can_create: false,
        can_destroy: false,
        can_map: false,
        can_unmap: false,
        can_grant: false,
        is_admin: false,
    };
    
    /// Read-write (send + receive)
    pub const READ_WRITE: Self = Self {
        can_send: true,
        can_receive: true,
        can_create: false,
        can_destroy: false,
        can_map: false,
        can_unmap: false,
        can_grant: false,
        is_admin: false,
    };
    
    /// Full admin access
    pub const ADMIN: Self = Self {
        can_send: true,
        can_receive: true,
        can_create: true,
        can_destroy: true,
        can_map: true,
        can_unmap: true,
        can_grant: true,
        is_admin: true,
    };
    
    /// Check if has specific capability
    pub fn has(&self, cap_type: CapabilityType) -> bool {
        match cap_type {
            CapabilityType::Send => self.can_send,
            CapabilityType::Receive => self.can_receive,
            CapabilityType::Create => self.can_create,
            CapabilityType::Destroy => self.can_destroy,
            CapabilityType::MapMemory => self.can_map,
            CapabilityType::UnmapMemory => self.can_unmap,
            CapabilityType::Grant => self.can_grant,
            CapabilityType::Admin => self.is_admin,
        }
    }
    
    /// Grant additional capability
    pub fn grant(&mut self, cap_type: CapabilityType) {
        match cap_type {
            CapabilityType::Send => self.can_send = true,
            CapabilityType::Receive => self.can_receive = true,
            CapabilityType::Create => self.can_create = true,
            CapabilityType::Destroy => self.can_destroy = true,
            CapabilityType::MapMemory => self.can_map = true,
            CapabilityType::UnmapMemory => self.can_unmap = true,
            CapabilityType::Grant => self.can_grant = true,
            CapabilityType::Admin => self.is_admin = true,
        }
    }
    
    /// Revoke capability
    pub fn revoke(&mut self, cap_type: CapabilityType) {
        match cap_type {
            CapabilityType::Send => self.can_send = false,
            CapabilityType::Receive => self.can_receive = false,
            CapabilityType::Create => self.can_create = false,
            CapabilityType::Destroy => self.can_destroy = false,
            CapabilityType::MapMemory => self.can_map = false,
            CapabilityType::UnmapMemory => self.can_unmap = false,
            CapabilityType::Grant => self.can_grant = false,
            CapabilityType::Admin => self.is_admin = false,
        }
    }
}

/// IPC capability token
pub struct Capability {
    /// Unique capability ID
    pub id: CapabilityId,
    
    /// Owner process ID
    pub owner_pid: u64,
    
    /// Target object ID (channel, shared memory, etc.)
    pub target_id: u64,
    
    /// Permission flags
    pub flags: CapabilityFlags,
    
    /// Optional label for debugging
    pub label: Option<String>,
    
    /// Creation timestamp (cycles)
    pub created_at: u64,
    
    /// Expiration timestamp (0 = never expires)
    pub expires_at: u64,
}

impl Capability {
    /// Create new capability
    pub fn new(owner_pid: u64, target_id: u64, flags: CapabilityFlags) -> Self {
        static NEXT_CAP_ID: AtomicU64 = AtomicU64::new(1);
        
        Self {
            id: CapabilityId(NEXT_CAP_ID.fetch_add(1, Ordering::Relaxed)),
            owner_pid,
            target_id,
            flags,
            label: None,
            created_at: 0, // TODO: Get actual timestamp
            expires_at: 0,
        }
    }
    
    /// Create with label
    pub fn with_label(mut self, label: String) -> Self {
        self.label = Some(label);
        self
    }
    
    /// Set expiration
    pub fn with_expiration(mut self, expires_at: u64) -> Self {
        self.expires_at = expires_at;
        self
    }
    
    /// Check if capability has expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        self.expires_at != 0 && current_time > self.expires_at
    }
    
    /// Check if has specific permission
    pub fn has_permission(&self, cap_type: CapabilityType) -> bool {
        self.flags.has(cap_type)
    }
    
    /// Verify capability for operation
    pub fn verify(&self, cap_type: CapabilityType, current_time: u64) -> bool {
        !self.is_expired(current_time) && self.has_permission(cap_type)
    }
}

/// Capability grant record (for tracking who granted what to whom)
pub struct CapabilityGrant {
    /// Capability that was granted
    pub capability: Capability,
    
    /// Process that granted this capability
    pub granter_pid: u64,
    
    /// Process that received this capability
    pub grantee_pid: u64,
    
    /// Timestamp when granted
    pub granted_at: u64,
}

/// Process capability set
pub struct ProcessCapabilities {
    /// Process ID
    pub pid: u64,
    
    /// All capabilities owned by this process
    pub capabilities: Vec<Capability>,
}

impl ProcessCapabilities {
    pub fn new(pid: u64) -> Self {
        Self {
            pid,
            capabilities: Vec::new(),
        }
    }
    
    /// Add capability
    pub fn add(&mut self, cap: Capability) {
        self.capabilities.push(cap);
    }
    
    /// Remove capability by ID
    pub fn remove(&mut self, cap_id: CapabilityId) -> Option<Capability> {
        if let Some(pos) = self.capabilities.iter().position(|c| c.id == cap_id) {
            Some(self.capabilities.remove(pos))
        } else {
            None
        }
    }
    
    /// Find capability for target with specific permission
    pub fn find(&self, target_id: u64, cap_type: CapabilityType, current_time: u64) -> Option<&Capability> {
        self.capabilities.iter()
            .find(|c| c.target_id == target_id && c.verify(cap_type, current_time))
    }
    
    /// Check if has permission for target
    pub fn has_permission(&self, target_id: u64, cap_type: CapabilityType, current_time: u64) -> bool {
        self.find(target_id, cap_type, current_time).is_some()
    }
    
    /// Clean up expired capabilities
    pub fn cleanup_expired(&mut self, current_time: u64) {
        self.capabilities.retain(|c| !c.is_expired(current_time));
    }
}

use alloc::collections::BTreeMap;
use spin::Mutex;

static CAPABILITY_TABLES: Mutex<BTreeMap<u64, CapabilityTable>> = Mutex::new(BTreeMap::new());

fn get_or_create_table(pid: u64) -> CapabilityTable {
    let mut tables = CAPABILITY_TABLES.lock();
    tables.entry(pid)
        .or_insert_with(|| CapabilityTable::new(pid))
        .clone()
}

/// Check if process has permission for IPC operation
pub fn check_permission(pid: u64, target_id: u64, cap_type: CapabilityType) -> bool {
    let tables = CAPABILITY_TABLES.lock();
    
    if let Some(table) = tables.get(&pid) {
        // Get current timestamp (stub)
        let current_time = 0;
        table.has_permission(target_id, cap_type, current_time)
    } else {
        // No table = no permissions
        false
    }
}

/// Grant capability to process
pub fn grant_capability(from_pid: u64, to_pid: u64, target_id: u64, flags: CapabilityFlags) -> Result<CapabilityId, &'static str> {
    // Verify granter has Grant permission
    if !check_permission(from_pid, target_id, CapabilityType::Grant) && from_pid != 1 {
        return Err("Permission denied: granter lacks Grant capability");
    }
    
    // Create capability
    let cap = Capability::new(to_pid, target_id, flags);
    let cap_id = cap.id;
    
    // Add to grantee's table
    let mut tables = CAPABILITY_TABLES.lock();
    let table = tables.entry(to_pid)
        .or_insert_with(|| CapabilityTable::new(to_pid));
    
    table.capabilities.push(cap);
    
    log::info!("grant_capability: {} -> {}, target={}, id={}", 
        from_pid, to_pid, target_id, cap_id);
    Ok(cap_id)
}

/// Revoke capability
pub fn revoke_capability(pid: u64, cap_id: CapabilityId) -> Result<(), &'static str> {
    let mut tables = CAPABILITY_TABLES.lock();
    
    if let Some(table) = tables.get_mut(&pid) {
        table.capabilities.retain(|c| c.id != cap_id);
        log::info!("revoke_capability: removed {} from PID {}", cap_id, pid);
        Ok(())
    } else {
        Err("Process not found")
    }
}
