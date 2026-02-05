//! User and Group ID types
//!
//! Type-safe wrappers for Unix user and group IDs with zero runtime overhead.

use core::fmt;

/// User ID (type-safe u32 wrapper)
///
/// Represents a Unix user identifier. Uses standard Unix conventions:
/// - 0: root user
/// - 1-999: system users
/// - 1000+: regular users
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Uid(u32);

impl Uid {
    /// Root user ID (superuser)
    pub const ROOT: Self = Self(0);
    
    /// First regular user ID (non-system)
    pub const FIRST_USER: Self = Self(1000);
    
    /// Nobody user ID (unprivileged)
    pub const NOBODY: Self = Self(65534);
    
    /// Maximum valid UID
    pub const MAX: u32 = u32::MAX - 1;

    /// Create new UID
    #[inline(always)]
    pub const fn new(uid: u32) -> Self {
        Self(uid)
    }

    /// Get raw u32 value
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
    
    /// Get as usize
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Check if this is root user (UID 0)
    #[inline(always)]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Check if this is a system user (UID < 1000)
    #[inline(always)]
    pub const fn is_system(self) -> bool {
        self.0 < 1000
    }
    
    /// Check if this is a regular user (UID >= 1000, excluding nobody)
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.0 >= 1000 && self.0 != 65534
    }
    
    /// Check if this is nobody user
    #[inline(always)]
    pub const fn is_nobody(self) -> bool {
        self.0 == 65534
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uid> for u32 {
    #[inline(always)]
    fn from(uid: Uid) -> u32 {
        uid.as_u32()
    }
}

impl From<u32> for Uid {
    #[inline(always)]
    fn from(uid: u32) -> Self {
        Self::new(uid)
    }
}

/// Group ID (type-safe u32 wrapper)
///
/// Represents a Unix group identifier. Uses standard Unix conventions:
/// - 0: root group
/// - 1-999: system groups
/// - 1000+: regular groups
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Gid(u32);

impl Gid {
    /// Root group ID
    pub const ROOT: Self = Self(0);
    
    /// First regular group ID (non-system)
    pub const FIRST_GROUP: Self = Self(1000);
    
    /// Nobody group ID (unprivileged)
    pub const NOGROUP: Self = Self(65534);
    
    /// Maximum valid GID
    pub const MAX: u32 = u32::MAX - 1;

    /// Create new GID
    #[inline(always)]
    pub const fn new(gid: u32) -> Self {
        Self(gid)
    }

    /// Get raw u32 value
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
    
    /// Get as usize
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Check if this is root group (GID 0)
    #[inline(always)]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Check if this is a system group (GID < 1000)
    #[inline(always)]
    pub const fn is_system(self) -> bool {
        self.0 < 1000
    }
    
    /// Check if this is a regular group (GID >= 1000, excluding nogroup)
    #[inline(always)]
    pub const fn is_group(self) -> bool {
        self.0 >= 1000 && self.0 != 65534
    }
    
    /// Check if this is nogroup
    #[inline(always)]
    pub const fn is_nogroup(self) -> bool {
        self.0 == 65534
    }
}

impl fmt::Display for Gid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Gid> for u32 {
    #[inline(always)]
    fn from(gid: Gid) -> u32 {
        gid.as_u32()
    }
}

impl From<u32> for Gid {
    #[inline(always)]
    fn from(gid: u32) -> Self {
        Self::new(gid)
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;

    #[test]
    fn test_uid_creation() {
        let uid = Uid::new(1000);
        assert_eq!(uid.as_u32(), 1000);
    }
    
    #[test]
    fn test_uid_constants() {
        assert_eq!(Uid::ROOT.as_u32(), 0);
        assert_eq!(Uid::FIRST_USER.as_u32(), 1000);
        assert_eq!(Uid::NOBODY.as_u32(), 65534);
    }

    #[test]
    fn test_uid_is_root() {
        assert!(Uid::ROOT.is_root());
        assert!(!Uid::new(1).is_root());
        assert!(!Uid::new(1000).is_root());
    }
    
    #[test]
    fn test_uid_is_system() {
        assert!(Uid::ROOT.is_system());
        assert!(Uid::new(1).is_system());
        assert!(Uid::new(999).is_system());
        assert!(!Uid::new(1000).is_system());
    }
    
    #[test]
    fn test_uid_is_user() {
        assert!(!Uid::ROOT.is_user());
        assert!(!Uid::new(999).is_user());
        assert!(Uid::new(1000).is_user());
        assert!(Uid::new(2000).is_user());
        assert!(!Uid::NOBODY.is_user());
    }
    
    #[test]
    fn test_uid_is_nobody() {
        assert!(!Uid::ROOT.is_nobody());
        assert!(!Uid::new(1000).is_nobody());
        assert!(Uid::NOBODY.is_nobody());
    }
    
    #[test]
    fn test_uid_conversions() {
        let uid = Uid::new(1000);
        assert_eq!(uid.as_u32(), 1000);
        assert_eq!(uid.as_usize(), 1000);
        
        let u32_val: u32 = uid.into();
        assert_eq!(u32_val, 1000);
        
        let from_u32 = Uid::from(1000u32);
        assert_eq!(from_u32.as_u32(), 1000);
    }
    
    #[test]
    fn test_uid_display() {
        let uid = Uid::new(1000);
        let s = std::format!("{}", uid);
        assert_eq!(s, "1000");
    }
    
    #[test]
    fn test_uid_ordering() {
        let uid1 = Uid::new(100);
        let uid2 = Uid::new(200);
        let uid3 = Uid::new(100);
        
        assert!(uid1 < uid2);
        assert!(uid2 > uid1);
        assert_eq!(uid1, uid3);
    }

    #[test]
    fn test_gid_creation() {
        let gid = Gid::new(1000);
        assert_eq!(gid.as_u32(), 1000);
    }
    
    #[test]
    fn test_gid_constants() {
        assert_eq!(Gid::ROOT.as_u32(), 0);
        assert_eq!(Gid::FIRST_GROUP.as_u32(), 1000);
        assert_eq!(Gid::NOGROUP.as_u32(), 65534);
    }

    #[test]
    fn test_gid_is_root() {
        assert!(Gid::ROOT.is_root());
        assert!(!Gid::new(1).is_root());
        assert!(!Gid::new(1000).is_root());
    }
    
    #[test]
    fn test_gid_is_system() {
        assert!(Gid::ROOT.is_system());
        assert!(Gid::new(1).is_system());
        assert!(Gid::new(999).is_system());
        assert!(!Gid::new(1000).is_system());
    }
    
    #[test]
    fn test_gid_is_group() {
        assert!(!Gid::ROOT.is_group());
        assert!(!Gid::new(999).is_group());
        assert!(Gid::new(1000).is_group());
        assert!(Gid::new(2000).is_group());
        assert!(!Gid::NOGROUP.is_group());
    }
    
    #[test]
    fn test_gid_is_nogroup() {
        assert!(!Gid::ROOT.is_nogroup());
        assert!(!Gid::new(1000).is_nogroup());
        assert!(Gid::NOGROUP.is_nogroup());
    }
    
    #[test]
    fn test_gid_conversions() {
        let gid = Gid::new(1000);
        assert_eq!(gid.as_u32(), 1000);
        assert_eq!(gid.as_usize(), 1000);
        
        let u32_val: u32 = gid.into();
        assert_eq!(u32_val, 1000);
        
        let from_u32 = Gid::from(1000u32);
        assert_eq!(from_u32.as_u32(), 1000);
    }
    
    #[test]
    fn test_gid_display() {
        let gid = Gid::new(1000);
        let s = std::format!("{}", gid);
        assert_eq!(s, "1000");
    }
    
    #[test]
    fn test_gid_ordering() {
        let gid1 = Gid::new(100);
        let gid2 = Gid::new(200);
        let gid3 = Gid::new(100);
        
        assert!(gid1 < gid2);
        assert!(gid2 > gid1);
        assert_eq!(gid1, gid3);
    }
    
    #[test]
    fn test_uid_size() {
        assert_eq!(size_of::<Uid>(), size_of::<u32>());
    }
    
    #[test]
    fn test_gid_size() {
        assert_eq!(size_of::<Gid>(), size_of::<u32>());
    }
    
    #[test]
    fn test_uid_copy() {
        let uid1 = Uid::new(1000);
        let uid2 = uid1;
        assert_eq!(uid1.as_u32(), uid2.as_u32());
    }
    
    #[test]
    fn test_gid_copy() {
        let gid1 = Gid::new(1000);
        let gid2 = gid1;
        assert_eq!(gid1.as_u32(), gid2.as_u32());
    }
}
