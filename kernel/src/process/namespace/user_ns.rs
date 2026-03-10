// kernel/src/process/namespace/user_ns.rs
//
// Espace de noms utilisateur (CLONE_NEWUSER) — Exo-OS Couche 1.5


use core::sync::atomic::{AtomicU32, Ordering};

/// Mapping uid/gid dans un user namespace.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct IdMapping {
    /// Première UID dans le namespace.
    pub ns_start:   u32,
    /// Première UID dans le parent.
    pub host_start: u32,
    /// Nombre d'UIDs mappées.
    pub count:      u32,
}

const MAX_ID_MAPS: usize = 5;

/// Espace de noms utilisateur.
#[repr(C)]
pub struct UserNamespace {
    pub id:         u32,
    pub refcount:   AtomicU32,
    pub valid:      AtomicU32,
    /// Parent user_ns (0 = racine).
    pub parent_id:  u32,
    /// Owner UID dans le parent.
    pub owner_uid:  u32,
    /// Owner GID dans le parent.
    pub owner_gid:  u32,
    /// Tableau des mappings UID.
    pub uid_maps:   [IdMapping; MAX_ID_MAPS],
    pub uid_map_count: AtomicU32,
    /// Tableau des mappings GID.
    pub gid_maps:   [IdMapping; MAX_ID_MAPS],
    pub gid_map_count: AtomicU32,
}

impl UserNamespace {
    const fn new_root() -> Self {
        // Dans le namespace racine : UID 0..0xFFFF_FFFF = identité.
        let identity_map = IdMapping {
            ns_start:   0,
            host_start: 0,
            count:      0xFFFF_FFFF,
        };
        let empty_map = IdMapping { ns_start: 0, host_start: 0, count: 0 };
        Self {
            id:          0,
            refcount:    AtomicU32::new(1),
            valid:       AtomicU32::new(1),
            parent_id:   0,
            owner_uid:   0,
            owner_gid:   0,
            uid_maps: [
                identity_map, empty_map, empty_map, empty_map, empty_map
            ],
            uid_map_count: AtomicU32::new(1),
            gid_maps: [
                identity_map, empty_map, empty_map, empty_map, empty_map
            ],
            gid_map_count: AtomicU32::new(1),
        }
    }

    /// Traduit une UID namespace → UID hôte.
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

    /// Traduit une UID hôte → UID namespace.
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

    pub fn inc_ref(&self) { self.refcount.fetch_add(1, Ordering::Relaxed); }
    pub fn dec_ref(&self) -> u32 { self.refcount.fetch_sub(1, Ordering::AcqRel) }
}

unsafe impl Sync for UserNamespace {}

/// Namespace utilisateur racine (UID 0 = root absolu).
pub static ROOT_USER_NS: UserNamespace = UserNamespace::new_root();
