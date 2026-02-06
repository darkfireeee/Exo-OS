//! User ID and Group ID types
//!
//! Type-safe wrappers for UID/GID with validation and privilege checks.

use core::fmt;

/// Root user ID (always 0)
pub const ROOT_UID: Uid = Uid(0);

/// Root group ID (always 0)
pub const ROOT_GID: Gid = Gid(0);

/// Nobody user ID (typically 65534)
pub const NOBODY_UID: Uid = Uid(65534);

/// Nobody group ID (typically 65534)
pub const NOBODY_GID: Gid = Gid(65534);

/// User ID
///
/// Type-safe wrapper for user identifiers.
/// On Unix-like systems, UIDs are unsigned 32-bit integers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Uid(u32);

impl Uid {
    /// Create a new UID
    #[inline(always)]
    pub const fn new(uid: u32) -> Self {
        Uid(uid)
    }

    /// Get raw UID value
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Get as usize
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Check if this is the root UID (0)
    #[inline(always)]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Check if this is a system UID (< 1000 on most systems)
    #[inline(always)]
    pub const fn is_system(self) -> bool {
        self.0 < 1000
    }

    /// Check if this is a regular user UID (>= 1000, < 65534)
    #[inline(always)]
    pub const fn is_regular_user(self) -> bool {
        self.0 >= 1000 && self.0 < 65534
    }

    /// Check if this is the nobody UID
    #[inline(always)]
    pub const fn is_nobody(self) -> bool {
        self.0 == 65534
    }

    /// Check if UID has elevated privileges (root or system)
    #[inline(always)]
    pub const fn is_privileged(self) -> bool {
        self.is_root() || self.is_system()
    }
}

impl fmt::Debug for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            f.write_str("Uid(ROOT)")
        } else if self.is_nobody() {
            f.write_str("Uid(NOBODY)")
        } else {
            f.debug_tuple("Uid").field(&self.0).finish()
        }
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for Uid {
    #[inline(always)]
    fn from(uid: u32) -> Self {
        Uid(uid)
    }
}

impl From<Uid> for u32 {
    #[inline(always)]
    fn from(uid: Uid) -> Self {
        uid.0
    }
}

impl From<Uid> for usize {
    #[inline(always)]
    fn from(uid: Uid) -> Self {
        uid.0 as usize
    }
}

/// Group ID
///
/// Type-safe wrapper for group identifiers.
/// On Unix-like systems, GIDs are unsigned 32-bit integers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Gid(u32);

impl Gid {
    /// Create a new GID
    #[inline(always)]
    pub const fn new(gid: u32) -> Self {
        Gid(gid)
    }

    /// Get raw GID value
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Get as usize
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Check if this is the root GID (0)
    #[inline(always)]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Check if this is a system GID (< 1000 on most systems)
    #[inline(always)]
    pub const fn is_system(self) -> bool {
        self.0 < 1000
    }

    /// Check if this is a regular user GID (>= 1000, < 65534)
    #[inline(always)]
    pub const fn is_regular_user(self) -> bool {
        self.0 >= 1000 && self.0 < 65534
    }

    /// Check if this is the nobody GID
    #[inline(always)]
    pub const fn is_nobody(self) -> bool {
        self.0 == 65534
    }

    /// Check if GID has elevated privileges (root or system)
    #[inline(always)]
    pub const fn is_privileged(self) -> bool {
        self.is_root() || self.is_system()
    }
}

impl fmt::Debug for Gid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            f.write_str("Gid(ROOT)")
        } else if self.is_nobody() {
            f.write_str("Gid(NOBODY)")
        } else {
            f.debug_tuple("Gid").field(&self.0).finish()
        }
    }
}

impl fmt::Display for Gid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for Gid {
    #[inline(always)]
    fn from(gid: u32) -> Self {
        Gid(gid)
    }
}

impl From<Gid> for u32 {
    #[inline(always)]
    fn from(gid: Gid) -> Self {
        gid.0
    }
}

impl From<Gid> for usize {
    #[inline(always)]
    fn from(gid: Gid) -> Self {
        gid.0 as usize
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;

    // ===== Uid Tests =====

    #[test]
    fn test_uid_creation() {
        let uid = Uid::new(1000);
        assert_eq!(uid.as_u32(), 1000);

        assert_eq!(Uid::new(0), ROOT_UID);
        assert_eq!(Uid::new(65534), NOBODY_UID);
    }

    #[test]
    fn test_uid_root_check() {
        assert!(ROOT_UID.is_root());
        assert!(!Uid::new(1000).is_root());
        assert!(!NOBODY_UID.is_root());
    }

    #[test]
    fn test_uid_system_check() {
        assert!(ROOT_UID.is_system());
        assert!(Uid::new(999).is_system());
        assert!(!Uid::new(1000).is_system());
        assert!(!Uid::new(5000).is_system());
    }

    #[test]
    fn test_uid_regular_user_check() {
        assert!(!ROOT_UID.is_regular_user());
        assert!(!Uid::new(999).is_regular_user());
        assert!(Uid::new(1000).is_regular_user());
        assert!(Uid::new(5000).is_regular_user());
        assert!(!NOBODY_UID.is_regular_user());
    }

    #[test]
    fn test_uid_nobody_check() {
        assert!(!ROOT_UID.is_nobody());
        assert!(!Uid::new(1000).is_nobody());
        assert!(NOBODY_UID.is_nobody());
    }

    #[test]
    fn test_uid_privileged_check() {
        assert!(ROOT_UID.is_privileged());
        assert!(Uid::new(500).is_privileged());
        assert!(!Uid::new(1000).is_privileged());
        assert!(!NOBODY_UID.is_privileged());
    }

    #[test]
    fn test_uid_conversions() {
        let uid = Uid::new(1000);

        assert_eq!(uid.as_u32(), 1000);
        assert_eq!(uid.as_usize(), 1000);

        let u32_val: u32 = uid.into();
        assert_eq!(u32_val, 1000);

        let usize_val: usize = uid.into();
        assert_eq!(usize_val, 1000);

        let from_u32 = Uid::from(1000);
        assert_eq!(from_u32, uid);
    }

    #[test]
    fn test_uid_display() {
        assert_eq!(std::format!("{}", Uid::new(1000)), "1000");

        let debug = std::format!("{:?}", ROOT_UID);
        assert!(debug.contains("ROOT"));

        let debug = std::format!("{:?}", NOBODY_UID);
        assert!(debug.contains("NOBODY"));
    }

    #[test]
    fn test_uid_ordering() {
        let uid1 = Uid::new(100);
        let uid2 = Uid::new(200);
        let uid3 = Uid::new(100);

        assert!(uid1 < uid2);
        assert!(uid2 > uid1);
        assert_eq!(uid1, uid3);
        assert_ne!(uid1, uid2);
    }

    #[test]
    fn test_uid_size() {
        assert_eq!(size_of::<Uid>(), size_of::<u32>());
    }

    // ===== Gid Tests =====

    #[test]
    fn test_gid_creation() {
        let gid = Gid::new(1000);
        assert_eq!(gid.as_u32(), 1000);

        assert_eq!(Gid::new(0), ROOT_GID);
        assert_eq!(Gid::new(65534), NOBODY_GID);
    }

    #[test]
    fn test_gid_root_check() {
        assert!(ROOT_GID.is_root());
        assert!(!Gid::new(1000).is_root());
        assert!(!NOBODY_GID.is_root());
    }

    #[test]
    fn test_gid_system_check() {
        assert!(ROOT_GID.is_system());
        assert!(Gid::new(999).is_system());
        assert!(!Gid::new(1000).is_system());
        assert!(!Gid::new(5000).is_system());
    }

    #[test]
    fn test_gid_regular_user_check() {
        assert!(!ROOT_GID.is_regular_user());
        assert!(!Gid::new(999).is_regular_user());
        assert!(Gid::new(1000).is_regular_user());
        assert!(Gid::new(5000).is_regular_user());
        assert!(!NOBODY_GID.is_regular_user());
    }

    #[test]
    fn test_gid_nobody_check() {
        assert!(!ROOT_GID.is_nobody());
        assert!(!Gid::new(1000).is_nobody());
        assert!(NOBODY_GID.is_nobody());
    }

    #[test]
    fn test_gid_privileged_check() {
        assert!(ROOT_GID.is_privileged());
        assert!(Gid::new(500).is_privileged());
        assert!(!Gid::new(1000).is_privileged());
        assert!(!NOBODY_GID.is_privileged());
    }

    #[test]
    fn test_gid_conversions() {
        let gid = Gid::new(1000);

        assert_eq!(gid.as_u32(), 1000);
        assert_eq!(gid.as_usize(), 1000);

        let u32_val: u32 = gid.into();
        assert_eq!(u32_val, 1000);

        let usize_val: usize = gid.into();
        assert_eq!(usize_val, 1000);

        let from_u32 = Gid::from(1000);
        assert_eq!(from_u32, gid);
    }

    #[test]
    fn test_gid_display() {
        assert_eq!(std::format!("{}", Gid::new(1000)), "1000");

        let debug = std::format!("{:?}", ROOT_GID);
        assert!(debug.contains("ROOT"));

        let debug = std::format!("{:?}", NOBODY_GID);
        assert!(debug.contains("NOBODY"));
    }

    #[test]
    fn test_gid_ordering() {
        let gid1 = Gid::new(100);
        let gid2 = Gid::new(200);
        let gid3 = Gid::new(100);

        assert!(gid1 < gid2);
        assert!(gid2 > gid1);
        assert_eq!(gid1, gid3);
        assert_ne!(gid1, gid2);
    }

    #[test]
    fn test_gid_size() {
        assert_eq!(size_of::<Gid>(), size_of::<u32>());
    }
}
