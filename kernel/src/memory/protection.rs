//! Page protection flags

use super::{VirtualAddress, MemoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageProtection(pub u8);

impl PageProtection {
    pub const READ_FLAG: u8 = 0x1;
    pub const WRITE_FLAG: u8 = 0x2;
    pub const EXECUTE_FLAG: u8 = 0x4;
    pub const USER_FLAG: u8 = 0x8;
    
    pub const READ: Self = Self(0x1);
    pub const WRITE: Self = Self(0x2);
    pub const EXECUTE: Self = Self(0x4);
    pub const USER: Self = Self(0x8);
    pub const READ_WRITE: Self = Self(0x3);
    pub const READ_EXECUTE: Self = Self(0x5);
    pub const READ_WRITE_EXECUTE: Self = Self(0x7);
    
    pub const fn new() -> Self {
        Self(0)
    }
    
    /// Create from POSIX PROT_* flags
    pub const fn from_prot(prot: u32) -> Self {
        let mut flags = 0u8;
        if prot & 0x1 != 0 { flags |= Self::READ_FLAG; }   // PROT_READ
        if prot & 0x2 != 0 { flags |= Self::WRITE_FLAG; }  // PROT_WRITE
        if prot & 0x4 != 0 { flags |= Self::EXECUTE_FLAG; } // PROT_EXEC
        Self(flags)
    }
    
    pub const fn read(self) -> Self {
        Self(self.0 | Self::READ_FLAG)
    }
    
    pub const fn write(self) -> Self {
        Self(self.0 | Self::WRITE_FLAG)
    }
    
    pub const fn execute(self) -> Self {
        Self(self.0 | Self::EXECUTE_FLAG)
    }
    
    pub const fn user(self) -> Self {
        Self(self.0 | Self::USER_FLAG)
    }
    
    pub const fn can_write(&self) -> bool {
        self.0 & Self::WRITE_FLAG != 0
    }
    
    pub const fn can_execute(&self) -> bool {
        self.0 & Self::EXECUTE_FLAG != 0
    }
    
    pub const fn is_user(&self) -> bool {
        self.0 & Self::USER_FLAG != 0
    }
    
    /// Alias for can_write
    pub const fn is_writable(&self) -> bool {
        self.0 & Self::WRITE_FLAG != 0
    }
    
    /// Alias for can_execute
    pub const fn is_executable(&self) -> bool {
        self.0 & Self::EXECUTE_FLAG != 0
    }
    
    /// Check if readable
    pub const fn is_readable(&self) -> bool {
        self.0 & Self::READ_FLAG != 0
    }
}

pub fn handle_protection_violation(_addr: VirtualAddress) -> Result<(), MemoryError> {
    // Stub pour gestion des violations
    Err(MemoryError::PermissionDenied)
}
