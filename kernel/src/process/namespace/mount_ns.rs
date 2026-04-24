// kernel/src/process/namespace/mount_ns.rs
//
// Espace de noms de montage (CLONE_NEWNS) — Exo-OS Couche 1.5
// Note : l'arbre de montage réel est implémenté dans fs/ (Couche 3).
// Ce module conserve uniquement l'identité et les références.

use core::sync::atomic::{AtomicU32, Ordering};

/// Espace de noms de montage.
#[repr(C)]
pub struct MountNamespace {
    /// Identifiant unique.
    pub id: u32,
    /// Namespace parent (0 = racine).
    pub parent_id: u32,
    /// Nombre de processus utilisant ce namespace.
    pub refcount: AtomicU32,
    /// Pointeur opaque vers la racine VFS (résolu par fs/).
    pub vfs_root_ptr: AtomicU32,
    /// Validité.
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

/// Namespace de montage racine (partagé par tous les processus init).
pub static ROOT_MOUNT_NS: MountNamespace = MountNamespace::new_root();
