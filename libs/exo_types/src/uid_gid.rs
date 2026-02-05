//! User and Group ID types

use core::fmt;

/// User ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Uid(u32);

impl Uid {
    /// Root user ID
    pub const ROOT: Uid = Uid(0);
    
    /// First user ID (non-system)
    pub const FIRST_USER: Uid = Uid(1000);

    /// Create new UID
    #[inline]
    pub const fn new(uid: u32) -> Self {
        Uid(uid)
    }

    /// Get raw value
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Check if root
    #[inline]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Check if system user
    #[inline]
    pub const fn is_system(self) -> bool {
        self.0 < 1000
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Group ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Gid(u32);

impl Gid {
    /// Root group ID
    pub const ROOT: Gid = Gid(0);
    
    /// First group ID (non-system)
    pub const FIRST_GROUP: Gid = Gid(1000);

    /// Create new GID
    #[inline]
    pub const fn new(gid: u32) -> Self {
        Gid(gid)
    }

    /// Get raw value
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Check if root group
    #[inline]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Check if system group
    #[inline]
    pub const fn is_system(self) -> bool {
        self.0 < 1000
    }
}

impl fmt::Display for Gid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uid() {
        assert!(Uid::ROOT.is_root());
        assert!(Uid::ROOT.is_system());
        assert!(!Uid::new(1000).is_system());
        assert!(!Uid::new(1000).is_root());
    }

    #[test]
    fn test_gid() {
        assert!(Gid::ROOT.is_root());
        assert!(Gid::ROOT.is_system());
        assert!(!Gid::new(1000).is_system());
    }
}
