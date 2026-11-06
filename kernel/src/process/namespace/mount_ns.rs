// kernel/src/process/namespace/mount_ns.rs
//
// Mount namespace (CLONE_NEWNS) — Exo-OS Layer 1.5
// Note: the actual mount tree is implemented in fs/ (Layer 3).
// This module only retains identity and references.

use core::sync::atomic::{AtomicU32, Ordering};

/// Mount namespace.
#[repr(C)]
pub struct MountNamespace {
    /// Unique identifier.
    pub id: u32,
    /// Parent namespace (0 = root).
    pub parent_id: u32,
    /// Number of processes using this namespace.
    pub refcount: AtomicU32,
    /// Opaque pointer to VFS root (resolved by fs/).
    pub vfs_root_ptr: AtomicU32,
    /// Validity.
    pub valid: AtomicU32,
}

impl MountNamespace {
    const fn new_root() -> Self {
        Self {
            id: 0,
            parent_id: 0,
            refcount: AtomicU32::new(1),
            vfs_root_ptr: AtomicU32::new(0),
            valid: AtomicU32::new(1),
        }
    }

    pub fn inc_ref(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_ref(&self) -> u32 {
        self.refcount.fetch_sub(1, Ordering::AcqRel)
    }
}

unsafe impl Sync for MountNamespace {}

/// Root mount namespace (shared by all init processes).
pub static ROOT_MOUNT_NS: MountNamespace = MountNamespace::new_root();
