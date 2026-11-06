// kernel/src/process/namespace/user_ns.rs
//
// User namespace (CLONE_NEWUSER) — Exo-OS Layer 1.5

use core::sync::atomic::{AtomicU32, Ordering};

/// uid/gid mapping in a user namespace.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct IdMapping {
    /// First UID in namespace.
    pub ns_start: u32,
    /// First UID in parent.
    pub host_start: u32,
    /// Number of mapped UIDs.
    pub count: u32,
}

const MAX_ID_MAPS: usize = 5;

/// User namespace.
#[repr(C)]
pub struct UserNamespace {
    pub id: u32,
    pub refcount: AtomicU32,
    pub valid: AtomicU32,
    /// Parent user_ns (0 = root).
    pub parent_id: u32,
    /// Owner UID in parent.
    pub owner_uid: u32,
    /// Owner GID in parent.
    pub owner_gid: u32,
    /// UID mapping table.
    pub uid_maps: [IdMapping; MAX_ID_MAPS],
    pub uid_map_count: AtomicU32,
    /// GID mapping table.
    pub gid_maps: [IdMapping; MAX_ID_MAPS],
    pub gid_map_count: AtomicU32,
}

impl UserNamespace {
    const fn new_root() -> Self {
        // In root namespace: UID 0..0xFFFF_FFFF = identity.
        let identity_map = IdMapping {
            ns_start: 0,
            host_start: 0,
            count: 0xFFFF_FFFF,
        };
        let empty_map = IdMapping {
            ns_start: 0,
            host_start: 0,
            count: 0,
        };
        Self {
            id: 0,
            refcount: AtomicU32::new(1),
            valid: AtomicU32::new(1),
            parent_id: 0,
            owner_uid: 0,
            owner_gid: 0,
            uid_maps: [identity_map, empty_map, empty_map, empty_map, empty_map],
            uid_map_count: AtomicU32::new(1),
            gid_maps: [identity_map, empty_map, empty_map, empty_map, empty_map],
            gid_map_count: AtomicU32::new(1),
        }
    }

    /// Translates a namespace UID → host UID.
    pub fn map_uid_to_host(&self, ns_uid: u32) -> Option<u32> {
        let count = self.uid_map_count.load(Ordering::Acquire) as usize;
        for map in &self.uid_maps[..count] {
            if ns_uid >= map.ns_start {
                let off = ns_uid - map.ns_start;
                if off < map.count {
                    return Some(map.host_start + off);
                }
            }
        }
        None
    }

    /// Translates a host UID → namespace UID.
    pub fn map_host_to_uid(&self, host_uid: u32) -> Option<u32> {
        let count = self.uid_map_count.load(Ordering::Acquire) as usize;
        for map in &self.uid_maps[..count] {
            if host_uid >= map.host_start {
                let off = host_uid - map.host_start;
                if off < map.count {
                    return Some(map.ns_start + off);
                }
            }
        }
        None
    }

    pub fn inc_ref(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_ref(&self) -> u32 {
        self.refcount.fetch_sub(1, Ordering::AcqRel)
    }
}

unsafe impl Sync for UserNamespace {}

/// Root user namespace (UID 0 = absolute root).
pub static ROOT_USER_NS: UserNamespace = UserNamespace::new_root();
