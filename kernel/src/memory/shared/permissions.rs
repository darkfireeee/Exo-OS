//! Shared memory permissions (POSIX-style)

use core::fmt;

/// Shared memory permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShmPermissions(pub u16);

impl ShmPermissions {
    // Owner permissions
    pub const OWNER_READ: u16 = 0o400;
    pub const OWNER_WRITE: u16 = 0o200;
    pub const OWNER_EXEC: u16 = 0o100;
    
    // Group permissions
    pub const GROUP_READ: u16 = 0o040;
    pub const GROUP_WRITE: u16 = 0o020;
    pub const GROUP_EXEC: u16 = 0o010;
    
    // Other permissions
    pub const OTHER_READ: u16 = 0o004;
    pub const OTHER_WRITE: u16 = 0o002;
    pub const OTHER_EXEC: u16 = 0o001;
    
    pub const fn new(mode: u16) -> Self {
        Self(mode)
    }
    
    pub const fn owner_can_read(&self) -> bool {
        self.0 & Self::OWNER_READ != 0
    }
    
    pub const fn owner_can_write(&self) -> bool {
        self.0 & Self::OWNER_WRITE != 0
    }
    
    pub const fn group_can_read(&self) -> bool {
        self.0 & Self::GROUP_READ != 0
    }
    
    pub const fn group_can_write(&self) -> bool {
        self.0 & Self::GROUP_WRITE != 0
    }
    
    pub const fn other_can_read(&self) -> bool {
        self.0 & Self::OTHER_READ != 0
    }
    
    pub const fn other_can_write(&self) -> bool {
        self.0 & Self::OTHER_WRITE != 0
    }
}

impl fmt::Display for ShmPermissions {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:03o}", self.0)
    }
}
