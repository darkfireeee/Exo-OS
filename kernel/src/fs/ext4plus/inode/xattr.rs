//! Extended Attributes (xattr)
//!
//! Support for extended attributes on inodes:
//! - user.* namespace
//! - trusted.* namespace
//! - security.* namespace (SELinux, capabilities)
//! - system.* namespace (ACLs, etc.)

use crate::fs::{FsError, FsResult};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// Extended attribute namespaces
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum XattrNamespace {
    User = 1,
    Trusted = 2,
    Security = 3,
    System = 4,
    PosixAcl = 5,
}

impl XattrNamespace {
    pub fn from_name(name: &str) -> Option<Self> {
        if name.starts_with("user.") {
            Some(XattrNamespace::User)
        } else if name.starts_with("trusted.") {
            Some(XattrNamespace::Trusted)
        } else if name.starts_with("security.") {
            Some(XattrNamespace::Security)
        } else if name.starts_with("system.") {
            Some(XattrNamespace::System)
        } else if name.starts_with("system.posix_acl_") {
            Some(XattrNamespace::PosixAcl)
        } else {
            None
        }
    }
}

/// Extended attribute
#[derive(Debug, Clone)]
pub struct ExtendedAttribute {
    /// Attribute name
    pub name: String,
    /// Attribute value
    pub value: Vec<u8>,
    /// Namespace
    pub namespace: XattrNamespace,
}

impl ExtendedAttribute {
    /// Create new xattr
    pub fn new(name: String, value: Vec<u8>) -> FsResult<Self> {
        let namespace = XattrNamespace::from_name(&name)
            .ok_or(FsError::InvalidArgument)?;

        Ok(Self {
            name,
            value,
            namespace,
        })
    }

    /// Get value as string
    pub fn value_string(&self) -> Option<String> {
        String::from_utf8(self.value.clone()).ok()
    }

    /// Get value size
    pub fn size(&self) -> usize {
        self.value.len()
    }
}

/// Extended Attribute Manager
pub struct XattrManager {
    /// In-memory xattr storage (inode -> name -> xattr)
    /// In production, would be stored in extended attribute blocks
    storage: spin::Mutex<BTreeMap<u64, BTreeMap<String, ExtendedAttribute>>>,
}

impl XattrManager {
    /// Create new xattr manager
    pub fn new() -> Self {
        Self {
            storage: spin::Mutex::new(BTreeMap::new()),
        }
    }

    /// Get extended attribute
    pub fn get(&self, ino: u64, name: &str) -> FsResult<Vec<u8>> {
        let storage = self.storage.lock();

        if let Some(inode_xattrs) = storage.get(&ino) {
            if let Some(xattr) = inode_xattrs.get(name) {
                return Ok(xattr.value.clone());
            }
        }

        Err(FsError::NotFound)
    }

    /// Set extended attribute
    pub fn set(&self, ino: u64, name: String, value: Vec<u8>) -> FsResult<()> {
        let xattr = ExtendedAttribute::new(name.clone(), value)?;

        let mut storage = self.storage.lock();
        let inode_xattrs = storage.entry(ino).or_insert_with(BTreeMap::new);
        inode_xattrs.insert(name, xattr);

        log::trace!("ext4plus: Set xattr for inode {}", ino);
        Ok(())
    }

    /// Remove extended attribute
    pub fn remove(&self, ino: u64, name: &str) -> FsResult<()> {
        let mut storage = self.storage.lock();

        if let Some(inode_xattrs) = storage.get_mut(&ino) {
            if inode_xattrs.remove(name).is_some() {
                log::trace!("ext4plus: Removed xattr for inode {}", ino);
                return Ok(());
            }
        }

        Err(FsError::NotFound)
    }

    /// List extended attributes
    pub fn list(&self, ino: u64) -> Vec<String> {
        let storage = self.storage.lock();

        if let Some(inode_xattrs) = storage.get(&ino) {
            inode_xattrs.keys().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Get all extended attributes for an inode
    pub fn get_all(&self, ino: u64) -> BTreeMap<String, Vec<u8>> {
        let storage = self.storage.lock();

        if let Some(inode_xattrs) = storage.get(&ino) {
            inode_xattrs.iter()
                .map(|(k, v)| (k.clone(), v.value.clone()))
                .collect()
        } else {
            BTreeMap::new()
        }
    }

    /// Check if xattr exists
    pub fn has(&self, ino: u64, name: &str) -> bool {
        let storage = self.storage.lock();

        if let Some(inode_xattrs) = storage.get(&ino) {
            inode_xattrs.contains_key(name)
        } else {
            false
        }
    }

    /// Clear all xattrs for an inode
    pub fn clear(&self, ino: u64) {
        let mut storage = self.storage.lock();
        storage.remove(&ino);
        log::trace!("ext4plus: Cleared all xattrs for inode {}", ino);
    }
}
