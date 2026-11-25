//! FNV-64a service naming registry for `ipc_router`.

use core::sync::atomic::{AtomicU32, Ordering};

pub const MAX_SERVICES: usize = 64;
const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
pub const fn fnv64a(name: &[u8]) -> u64 {
    let mut h = FNV64_OFFSET;
    let mut i = 0usize;
    while i < name.len() {
        h ^= name[i] as u64;
        h = h.wrapping_mul(FNV64_PRIME);
        i += 1;
    }
    h
}

/// Registre d'endpoints : max 64 services simultanés (FNV-64a).
pub struct Registry {
    names: [u64; MAX_SERVICES],
    endpoints: [u32; MAX_SERVICES],
    count: AtomicU32,
}

impl Registry {
    pub const fn new() -> Self {
        Self {
            names: [0u64; MAX_SERVICES],
            endpoints: [0u32; MAX_SERVICES],
            count: AtomicU32::new(0),
        }
    }

    pub const fn hash_name(name: &[u8]) -> u64 {
        fnv64a(name)
    }

    pub fn count(&self) -> usize {
        (self.count.load(Ordering::Acquire) as usize).min(MAX_SERVICES)
    }

    /// Enregistre ou met à jour un endpoint de manière idempotente. Retourne `false` si la table est pleine.
    pub fn register_hash(&mut self, h: u64, endpoint: u32) -> bool {
        let n = (self.count.load(Ordering::Relaxed) as usize).min(MAX_SERVICES);
        for i in 0..n {
            if self.names[i] == h {
                self.endpoints[i] = endpoint;
                return true;
            }
        }
        if n >= MAX_SERVICES {
            return false;
        }
        self.names[n] = h;
        self.endpoints[n] = endpoint;
        self.count.store((n + 1) as u32, Ordering::Release);
        true
    }

    pub fn register(&mut self, name: &[u8], endpoint: u32) -> bool {
        self.register_hash(Self::hash_name(name), endpoint)
    }

    pub fn resolve_hash(&self, h: u64) -> Option<u32> {
        let n = self.count();
        for i in 0..n {
            if self.names[i] == h {
                return Some(self.endpoints[i]);
            }
        }
        None
    }

    pub fn resolve(&self, name: &[u8]) -> Option<u32> {
        self.resolve_hash(Self::hash_name(name))
    }
}
