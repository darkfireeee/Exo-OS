//! VFS Directory Entry (dentry) representation.

use super::inode::Inode;
use alloc::string::String;
use alloc::sync::Arc;
use spin::RwLock;

/// Directory entry
pub struct Dentry {
    pub name: String,
    pub inode: Arc<RwLock<dyn Inode>>,
}

impl Dentry {
    pub fn new(name: String, inode: Arc<RwLock<dyn Inode>>) -> Self {
        Self { name, inode }
    }
}
