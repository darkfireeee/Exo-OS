//! Security Objects
//!
//! Kernel objects with security context

use core::sync::atomic::{AtomicU64, Ordering};

/// Object ID - unique identifier for any kernel object
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct ObjectId(pub u64);

impl ObjectId {
    pub const INVALID: Self = Self(0);

    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn from_fd(fd: i32) -> Self {
        // Map FD to object ID
        // Use high bit to distinguish from other object types
        Self(0x8000_0000_0000_0000 | (fd as u64))
    }

    pub fn from_inode(inode: u64) -> Self {
        // Inode-based object ID
        Self(0x4000_0000_0000_0000 | inode)
    }

    pub fn from_pid(pid: u32) -> Self {
        // Process-based object ID
        Self(0x2000_0000_0000_0000 | (pid as u64))
    }

    pub fn next(current: &AtomicU64) -> Self {
        Self(current.fetch_add(1, Ordering::Relaxed))
    }
}

/// Object type categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectType {
    File = 0,
    Directory = 1,
    Device = 2,
    CharDevice = 3,
    BlockDevice = 4,
    Pipe = 5,
    Socket = 6,
    Symlink = 7,
    Process = 8,
    Thread = 9,
    Memory = 10,
    IpcChannel = 11,
    Capability = 12,
    Handle = 13,
}

/// Object metadata
#[derive(Debug, Clone, Copy)]
pub struct ObjectMetadata {
    /// Owner user ID
    pub owner: u32,
    /// Owner group ID
    pub group: u32,
    /// Creation timestamp
    pub created_at: u64,
    /// Last access timestamp
    pub accessed_at: u64,
    /// Flags
    pub flags: u32,
}

impl ObjectMetadata {
    pub fn new(owner: u32, group: u32) -> Self {
        Self {
            owner,
            group,
            created_at: 0, // TODO: Real timestamp
            accessed_at: 0,
            flags: 0,
        }
    }
}

/// Kernel object with security context
#[derive(Debug, Clone)]
pub struct Object {
    /// Unique object ID
    pub id: ObjectId,
    /// Object type
    pub object_type: ObjectType,
    /// Metadata (owner, timestamps, etc.)
    pub metadata: ObjectMetadata,
}

impl Object {
    pub fn new(id: ObjectId, object_type: ObjectType, owner: u32, group: u32) -> Self {
        Self {
            id,
            object_type,
            metadata: ObjectMetadata::new(owner, group),
        }
    }

    pub fn owner(&self) -> u32 {
        self.metadata.owner
    }

    pub fn group(&self) -> u32 {
        self.metadata.group
    }

    pub fn is_owned_by(&self, uid: u32) -> bool {
        self.metadata.owner == uid
    }

    pub fn is_in_group(&self, gid: u32) -> bool {
        self.metadata.group == gid
    }
}
