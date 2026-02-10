//! Access Control Lists (ACL)
//!
//! POSIX ACL support for fine-grained permissions:
//! - Access ACLs (permissions for the file)
//! - Default ACLs (inherited by new files in directories)
//! - ACL entries for user, group, other, mask

use crate::fs::{FsError, FsResult};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// ACL entry tag
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AclTag {
    /// Owner permissions
    UserObj = 1,
    /// Named user permissions
    User = 2,
    /// Owning group permissions
    GroupObj = 3,
    /// Named group permissions
    Group = 4,
    /// Mask (maximum permissions)
    Mask = 5,
    /// Other permissions
    Other = 6,
}

/// ACL permissions
#[derive(Debug, Clone, Copy, Default)]
pub struct AclPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl AclPermissions {
    pub fn from_u16(perms: u16) -> Self {
        Self {
            read: (perms & 4) != 0,
            write: (perms & 2) != 0,
            execute: (perms & 1) != 0,
        }
    }

    pub fn to_u16(&self) -> u16 {
        let mut perms = 0u16;
        if self.read { perms |= 4; }
        if self.write { perms |= 2; }
        if self.execute { perms |= 1; }
        perms
    }

    pub fn all() -> Self {
        Self {
            read: true,
            write: true,
            execute: true,
        }
    }

    pub fn none() -> Self {
        Self {
            read: false,
            write: false,
            execute: false,
        }
    }
}

/// ACL entry
#[derive(Debug, Clone)]
pub struct AccessControlEntry {
    /// Entry tag
    pub tag: AclTag,
    /// Qualifier (uid/gid for User/Group tags)
    pub qualifier: Option<u32>,
    /// Permissions
    pub permissions: AclPermissions,
}

impl AccessControlEntry {
    /// Create new ACL entry
    pub fn new(tag: AclTag, qualifier: Option<u32>, permissions: AclPermissions) -> Self {
        Self {
            tag,
            qualifier,
            permissions,
        }
    }

    /// Create user ACL entry
    pub fn user(uid: u32, permissions: AclPermissions) -> Self {
        Self::new(AclTag::User, Some(uid), permissions)
    }

    /// Create group ACL entry
    pub fn group(gid: u32, permissions: AclPermissions) -> Self {
        Self::new(AclTag::Group, Some(gid), permissions)
    }

    /// Create owner ACL entry
    pub fn user_obj(permissions: AclPermissions) -> Self {
        Self::new(AclTag::UserObj, None, permissions)
    }

    /// Create owning group ACL entry
    pub fn group_obj(permissions: AclPermissions) -> Self {
        Self::new(AclTag::GroupObj, None, permissions)
    }

    /// Create mask ACL entry
    pub fn mask(permissions: AclPermissions) -> Self {
        Self::new(AclTag::Mask, None, permissions)
    }

    /// Create other ACL entry
    pub fn other(permissions: AclPermissions) -> Self {
        Self::new(AclTag::Other, None, permissions)
    }
}

/// Access Control List
#[derive(Debug, Clone)]
pub struct AccessControlList {
    /// ACL entries
    entries: Vec<AccessControlEntry>,
}

impl AccessControlList {
    /// Create new ACL
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Create minimal ACL from mode
    pub fn from_mode(mode: u16) -> Self {
        let mut acl = Self::new();

        acl.entries.push(AccessControlEntry::user_obj(
            AclPermissions::from_u16((mode >> 6) & 7)
        ));
        acl.entries.push(AccessControlEntry::group_obj(
            AclPermissions::from_u16((mode >> 3) & 7)
        ));
        acl.entries.push(AccessControlEntry::other(
            AclPermissions::from_u16(mode & 7)
        ));

        acl
    }

    /// Add ACL entry
    pub fn add_entry(&mut self, entry: AccessControlEntry) {
        self.entries.push(entry);
    }

    /// Remove ACL entry
    pub fn remove_entry(&mut self, tag: AclTag, qualifier: Option<u32>) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| {
            e.tag == tag && e.qualifier == qualifier
        }) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get entry
    pub fn get_entry(&self, tag: AclTag, qualifier: Option<u32>) -> Option<&AccessControlEntry> {
        self.entries.iter().find(|e| e.tag == tag && e.qualifier == qualifier)
    }

    /// Check access for user/group
    pub fn check_access(&self, uid: u32, gid: u32, groups: &[u32], requested: AclPermissions) -> bool {
        // Check user_obj first
        if let Some(entry) = self.get_entry(AclTag::UserObj, None) {
            if self.check_permissions(&entry.permissions, &requested) {
                return true;
            }
        }

        // Check named user entries
        if let Some(entry) = self.get_entry(AclTag::User, Some(uid)) {
            if self.check_permissions(&entry.permissions, &requested) {
                return true;
            }
        }

        // Check group_obj
        if let Some(entry) = self.get_entry(AclTag::GroupObj, None) {
            if self.check_permissions(&entry.permissions, &requested) {
                return true;
            }
        }

        // Check named group entries
        for &group in groups {
            if let Some(entry) = self.get_entry(AclTag::Group, Some(group)) {
                if self.check_permissions(&entry.permissions, &requested) {
                    return true;
                }
            }
        }

        // Check other
        if let Some(entry) = self.get_entry(AclTag::Other, None) {
            if self.check_permissions(&entry.permissions, &requested) {
                return true;
            }
        }

        false
    }

    /// Check if permissions satisfy request
    fn check_permissions(&self, have: &AclPermissions, need: &AclPermissions) -> bool {
        (!need.read || have.read) &&
        (!need.write || have.write) &&
        (!need.execute || have.execute)
    }

    /// Get all entries
    pub fn entries(&self) -> &[AccessControlEntry] {
        &self.entries
    }

    /// Validate ACL
    pub fn validate(&self) -> FsResult<()> {
        // Must have user_obj, group_obj, and other
        let has_user_obj = self.entries.iter().any(|e| e.tag == AclTag::UserObj);
        let has_group_obj = self.entries.iter().any(|e| e.tag == AclTag::GroupObj);
        let has_other = self.entries.iter().any(|e| e.tag == AclTag::Other);

        if !has_user_obj || !has_group_obj || !has_other {
            return Err(FsError::InvalidArgument);
        }

        // If there are named user/group entries, must have mask
        let has_named = self.entries.iter().any(|e| {
            e.tag == AclTag::User || e.tag == AclTag::Group
        });

        if has_named {
            let has_mask = self.entries.iter().any(|e| e.tag == AclTag::Mask);
            if !has_mask {
                return Err(FsError::InvalidArgument);
            }
        }

        Ok(())
    }
}

/// ACL Manager
pub struct AclManager {
    /// Access ACLs (inode -> ACL)
    access_acls: spin::Mutex<BTreeMap<u64, AccessControlList>>,
    /// Default ACLs (directory inode -> ACL)
    default_acls: spin::Mutex<BTreeMap<u64, AccessControlList>>,
}

impl AclManager {
    /// Create new ACL manager
    pub fn new() -> Self {
        Self {
            access_acls: spin::Mutex::new(BTreeMap::new()),
            default_acls: spin::Mutex::new(BTreeMap::new()),
        }
    }

    /// Get access ACL
    pub fn get_access_acl(&self, ino: u64) -> Option<AccessControlList> {
        let acls = self.access_acls.lock();
        acls.get(&ino).cloned()
    }

    /// Set access ACL
    pub fn set_access_acl(&self, ino: u64, acl: AccessControlList) -> FsResult<()> {
        acl.validate()?;

        let mut acls = self.access_acls.lock();
        acls.insert(ino, acl);

        log::trace!("ext4plus: Set access ACL for inode {}", ino);
        Ok(())
    }

    /// Remove access ACL
    pub fn remove_access_acl(&self, ino: u64) {
        let mut acls = self.access_acls.lock();
        acls.remove(&ino);
        log::trace!("ext4plus: Removed access ACL for inode {}", ino);
    }

    /// Get default ACL
    pub fn get_default_acl(&self, ino: u64) -> Option<AccessControlList> {
        let acls = self.default_acls.lock();
        acls.get(&ino).cloned()
    }

    /// Set default ACL
    pub fn set_default_acl(&self, ino: u64, acl: AccessControlList) -> FsResult<()> {
        acl.validate()?;

        let mut acls = self.default_acls.lock();
        acls.insert(ino, acl);

        log::trace!("ext4plus: Set default ACL for inode {}", ino);
        Ok(())
    }

    /// Remove default ACL
    pub fn remove_default_acl(&self, ino: u64) {
        let mut acls = self.default_acls.lock();
        acls.remove(&ino);
        log::trace!("ext4plus: Removed default ACL for inode {}", ino);
    }

    /// Check access
    pub fn check_access(&self, ino: u64, uid: u32, gid: u32, groups: &[u32], requested: AclPermissions) -> bool {
        // Root always has access
        if uid == 0 {
            return true;
        }

        if let Some(acl) = self.get_access_acl(ino) {
            acl.check_access(uid, gid, groups, requested)
        } else {
            // No ACL - use basic permissions
            false
        }
    }
}
