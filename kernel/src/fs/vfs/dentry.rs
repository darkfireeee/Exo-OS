//! VFS Directory Entry (dentry) representation.
//!
//! High-performance dentry implementation with:
//! - Parent/child linking for traversal
//! - State tracking (valid, deleted, revalidate)
//! - Negative dentry support (caching non-existent paths)
//! - Reference counting

use super::inode::Inode;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::RwLock;
use hashbrown::HashMap;

/// Dentry state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DentryState {
    /// Valid and cached
    Valid,
    /// Negative dentry (path doesn't exist, but cached to avoid repeated lookups)
    Negative,
    /// Needs revalidation (e.g., after NFS timeout)
    NeedsRevalidate,
    /// Deleted, waiting for cleanup
    Deleted,
}

/// Directory entry - links names to inodes
pub struct Dentry {
    /// Entry name (filename component)
    pub name: String,
    /// Inode this entry points to (None for negative dentries)
    pub inode: Option<Arc<RwLock<dyn Inode>>>,
    /// Inode number (for quick access without locking inode)
    pub ino: u64,
    /// Parent dentry (weak to avoid cycles)
    pub parent: Option<Weak<RwLock<Dentry>>>,
    /// Child dentries (for directories)
    pub children: HashMap<String, Arc<RwLock<Dentry>>>,
    /// Current state
    pub state: DentryState,
    /// Reference count for caching
    pub refcount: u32,
    /// Flags
    pub flags: DentryFlags,
}

/// Dentry flags
#[derive(Debug, Clone, Copy, Default)]
pub struct DentryFlags {
    /// Is this a mount point?
    pub is_mountpoint: bool,
    /// Is this an automount point?
    pub is_automount: bool,
    /// Has been accessed recently (for LRU)
    pub accessed: bool,
}

impl Dentry {
    /// Create a new dentry
    pub fn new(name: String, inode: Arc<RwLock<dyn Inode>>) -> Self {
        let ino = inode.read().ino();
        Self {
            name,
            inode: Some(inode),
            ino,
            parent: None,
            children: HashMap::new(),
            state: DentryState::Valid,
            refcount: 1,
            flags: DentryFlags::default(),
        }
    }
    
    /// Create a negative dentry (caching a non-existent path)
    pub fn new_negative(name: String) -> Self {
        Self {
            name,
            inode: None,
            ino: 0,
            parent: None,
            children: HashMap::new(),
            state: DentryState::Negative,
            refcount: 1,
            flags: DentryFlags::default(),
        }
    }
    
    /// Create root dentry
    pub fn root(inode: Arc<RwLock<dyn Inode>>) -> Self {
        let ino = inode.read().ino();
        Self {
            name: String::from("/"),
            inode: Some(inode),
            ino,
            parent: None,
            children: HashMap::new(),
            state: DentryState::Valid,
            refcount: 1,
            flags: DentryFlags::default(),
        }
    }
    
    /// Check if this is a valid (positive) dentry
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.state == DentryState::Valid && self.inode.is_some()
    }
    
    /// Check if this is a negative dentry
    #[inline]
    pub fn is_negative(&self) -> bool {
        self.state == DentryState::Negative
    }
    
    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        if let Some(ref inode) = self.inode {
            inode.read().inode_type() == super::inode::InodeType::Directory
        } else {
            false
        }
    }
    
    /// Add a child dentry
    pub fn add_child(&mut self, child: Arc<RwLock<Dentry>>) {
        let name = child.read().name.clone();
        self.children.insert(name, child);
    }
    
    /// Remove a child dentry
    pub fn remove_child(&mut self, name: &str) -> Option<Arc<RwLock<Dentry>>> {
        self.children.remove(name)
    }
    
    /// Lookup a child by name
    pub fn lookup_child(&self, name: &str) -> Option<Arc<RwLock<Dentry>>> {
        self.children.get(name).cloned()
    }
    
    /// Get full path by walking up to root
    pub fn full_path(&self) -> String {
        let mut components = Vec::new();
        components.push(self.name.clone());
        
        let mut current_parent = self.parent.clone();
        while let Some(weak_parent) = current_parent {
            if let Some(parent) = weak_parent.upgrade() {
                let parent_guard = parent.read();
                if parent_guard.name != "/" {
                    components.push(parent_guard.name.clone());
                }
                current_parent = parent_guard.parent.clone();
            } else {
                break;
            }
        }
        
        components.reverse();
        if components.len() == 1 && components[0] == "/" {
            String::from("/")
        } else {
            format!("/{}", components.join("/"))
        }
    }
    
    /// Increment reference count
    #[inline]
    pub fn get(&mut self) {
        self.refcount = self.refcount.saturating_add(1);
        self.flags.accessed = true;
    }
    
    /// Decrement reference count, returns true if should be freed
    #[inline]
    pub fn put(&mut self) -> bool {
        self.refcount = self.refcount.saturating_sub(1);
        self.refcount == 0
    }
    
    /// Mark as deleted
    pub fn mark_deleted(&mut self) {
        self.state = DentryState::Deleted;
    }
    
    /// Mark as needing revalidation
    pub fn mark_revalidate(&mut self) {
        self.state = DentryState::NeedsRevalidate;
    }
}
