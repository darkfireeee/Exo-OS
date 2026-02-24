// kernel/src/process/namespace/net_ns.rs
//
// Espace de noms réseau (CLONE_NEWNET) — Exo-OS Couche 1.5

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};

/// Identifiant d'un espace de noms réseau.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct NetNsId(pub u32);

/// Espace de noms réseau.
#[repr(C)]
pub struct NetNamespace {
    /// Identifiant unique.
    pub id:        u32,
    /// Nombre de processus.
    pub refcount:  AtomicU32,
    /// Loopback actif.
    pub loopback:  AtomicU32,
    /// Validité.
    pub valid:     AtomicU32,
}

impl NetNamespace {
    const fn new_root() -> Self {
        Self {
            id:       0,
            refcount: AtomicU32::new(1),
            loopback: AtomicU32::new(0),
            valid:    AtomicU32::new(1),
        }
    }

    pub fn inc_ref(&self) { self.refcount.fetch_add(1, Ordering::Relaxed); }
    pub fn dec_ref(&self) -> u32 { self.refcount.fetch_sub(1, Ordering::AcqRel) }
}

unsafe impl Sync for NetNamespace {}

/// Namespace réseau racine.
pub static ROOT_NET_NS: NetNamespace = NetNamespace::new_root();
