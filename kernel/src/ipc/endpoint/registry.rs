// kernel/src/ipc/endpoint/registry.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ENDPOINT REGISTRY — Name -> endpoint registry (static hash table)
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// The registry maintains an open-addressed hash table (open addressing,
// Robin Hood hashing) of active endpoints, indexed by name.
//
// IMPLEMENTATION:
//   • Static hash table (MAX_ENDPOINTS entries).
//   • 64-bit FNV-1a hash (fast, allocation-free).
//   • Robin Hood hashing to minimize probe variance.
//   • Protected by SpinLock (registry shared across all threads).
//   • Lookup O(1) average, O(n) worst-case (very unlikely with FNV-1a).
//
// NO-ALLOC ZONE RULE: zero Vec/Box/Arc in this file.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::ipc::core::constants::MAX_ENDPOINTS;
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// 64-bit FNV-1a Hash
// ─────────────────────────────────────────────────────────────────────────────

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a hash of a byte slice.
#[inline]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ─────────────────────────────────────────────────────────────────────────────
// RegistryEntry — an entry in the hash table
// ─────────────────────────────────────────────────────────────────────────────

/// Robin Hood displacement distance (0 = free slot/tombstone).
type RobinHoodDist = u16;

/// Hash table entry.
#[derive(Clone, Copy)]
struct RegistryEntry {
    /// Name hash (-1 if free slot, -2 = tombstone).
    hash: u64,
    /// Stored EndpointId.
    ep_id: u64,
    /// Robin Hood distance from ideal position.
    rh_dist: RobinHoodDist,
    /// Padding.
    _pad: [u8; 6],
}

impl RegistryEntry {
    const EMPTY: u64 = u64::MAX;
    const TOMBSTONE: u64 = u64::MAX - 1;

    const fn empty() -> Self {
        Self {
            hash: Self::EMPTY,
            ep_id: 0,
            rh_dist: 0,
            _pad: [0u8; 6],
        }
    }

    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.hash == Self::EMPTY
    }

    #[inline(always)]
    #[allow(dead_code)]
    fn is_tombstone(&self) -> bool {
        self.hash == Self::TOMBSTONE
    }

    #[inline(always)]
    #[allow(dead_code)]
    fn is_occupied(&self) -> bool {
        self.hash != Self::EMPTY && self.hash != Self::TOMBSTONE
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EndpointRegistry — global table
// ─────────────────────────────────────────────────────────────────────────────

/// Hash table of active endpoints.
struct RegistryInner {
    /// Table slots (open addressing, Robin Hood).
    slots: [RegistryEntry; MAX_ENDPOINTS],
    /// Number of occupied entries.
    count: usize,
}

impl RegistryInner {
    const fn new() -> Self {
        // SAFETY: RegistryEntry::empty() is a valid constant.
        const ZERO: RegistryEntry = RegistryEntry::empty();
        Self {
            slots: [ZERO; MAX_ENDPOINTS],
            count: 0,
        }
    }

    /// Deletes a slot via backward shift deletion,
    /// eliminating tombstones and preserving Robin Hood invariants.
    fn backward_shift_delete(&mut self, mut pos: usize) {
        loop {
            let next = (pos + 1) % MAX_ENDPOINTS;
            let next_slot = self.slots[next];
            if next_slot.is_empty() || next_slot.rh_dist == 0 {
                self.slots[pos] = RegistryEntry::empty();
                break;
            }
            self.slots[pos] = RegistryEntry {
                hash: next_slot.hash,
                ep_id: next_slot.ep_id,
                rh_dist: next_slot.rh_dist - 1,
                _pad: [0u8; 6],
            };
            pos = next;
        }
        self.count = self.count.saturating_sub(1);
    }

    /// Inserts or updates an endpoint.
    fn insert(&mut self, hash: u64, ep_id: u64) -> Result<(), IpcError> {
        if self.count >= MAX_ENDPOINTS * 3 / 4 {
            // Table at 75% capacity — reject to avoid excessive collisions.
            return Err(IpcError::ResourceExhausted);
        }
        let mut pos = (hash as usize) % MAX_ENDPOINTS;
        let dist: RobinHoodDist = 0;
        let mut cur_hash = hash;
        let mut cur_ep_id = ep_id;
        let mut cur_dist = dist;

        loop {
            let slot = &mut self.slots[pos];
            if slot.is_empty() {
                *slot = RegistryEntry {
                    hash: cur_hash,
                    ep_id: cur_ep_id,
                    rh_dist: cur_dist,
                    _pad: [0u8; 6],
                };
                self.count += 1;
                return Ok(());
            }
            if slot.hash == cur_hash {
                if slot.ep_id == cur_ep_id {
                    return Ok(());
                }
                return Err(IpcError::AlreadyConnected);
            }
            // Robin Hood: if existing slot is closer to its ideal position,
            // displace it.
            if slot.rh_dist < cur_dist {
                core::mem::swap(&mut slot.hash, &mut cur_hash);
                core::mem::swap(&mut slot.ep_id, &mut cur_ep_id);
                core::mem::swap(&mut slot.rh_dist, &mut cur_dist);
            }
            pos = (pos + 1) % MAX_ENDPOINTS;
            cur_dist += 1;
            if cur_dist as usize >= MAX_ENDPOINTS {
                return Err(IpcError::InternalError);
            }
        }
    }

    /// Looks up an endpoint by hash.
    /// Returns EndpointId or None if not found.
    fn lookup(&self, hash: u64) -> Option<u64> {
        let mut pos = (hash as usize) % MAX_ENDPOINTS;
        let mut dist = 0usize;
        loop {
            let slot = &self.slots[pos];
            if slot.is_empty() {
                return None;
            }
            if slot.hash == hash {
                return Some(slot.ep_id);
            }
            // Robin Hood: if current distance exceeds slot distance, requested element
            // cannot be present (Robin Hood sub-invariant).
            if (slot.rh_dist as usize) < dist {
                return None;
            }
            pos = (pos + 1) % MAX_ENDPOINTS;
            dist += 1;
            if dist >= MAX_ENDPOINTS {
                return None;
            }
        }
    }

    /// Removes an endpoint from registry.
    fn remove(&mut self, hash: u64) -> bool {
        let mut pos = (hash as usize) % MAX_ENDPOINTS;
        let mut dist = 0usize;
        loop {
            let slot = &self.slots[pos];
            if slot.is_empty() {
                return false;
            }
            if slot.hash == hash {
                self.backward_shift_delete(pos);
                return true;
            }
            if (slot.rh_dist as usize) < dist {
                return false;
            }
            pos = (pos + 1) % MAX_ENDPOINTS;
            dist += 1;
            if dist >= MAX_ENDPOINTS {
                return false;
            }
        }
    }

    /// Removes an endpoint from registry by its numeric identifier.
    fn remove_by_endpoint(&mut self, ep_id: u64) -> bool {
        for pos in 0..MAX_ENDPOINTS {
            if !self.slots[pos].is_empty() && self.slots[pos].ep_id == ep_id {
                self.backward_shift_delete(pos);
                return true;
            }
        }
        false
    }

    /// Returns number of registered endpoints.
    #[inline(always)]
    fn count(&self) -> usize {
        self.count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NamedEndpointRegistry — global, thread-safe registry
// ─────────────────────────────────────────────────────────────────────────────

/// Global IPC endpoint registry.
///
/// Thread-safe via SpinLock.
/// Lookup via FNV-1a hash on ASCII name.
pub struct NamedEndpointRegistry {
    inner: SpinLock<RegistryInner>,
    /// Total lookup counter (instrumentation).
    lookup_count: AtomicU64,
    /// Total insertion counter.
    insert_count: AtomicU64,
}

impl NamedEndpointRegistry {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(RegistryInner::new()),
            lookup_count: AtomicU64::new(0),
            insert_count: AtomicU64::new(0),
        }
    }

    /// Registers an endpoint by name.
    pub fn register(&self, name: &[u8], ep_id: EndpointId) -> Result<(), IpcError> {
        if name.is_empty() || name.len() >= crate::ipc::core::MAX_ENDPOINT_NAME_LEN {
            return Err(IpcError::InvalidParam);
        }
        let hash = fnv1a(name);
        let mut inner = self.inner.lock();
        inner.insert(hash, ep_id.get())?;
        self.insert_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Looks up an endpoint by name.
    pub fn lookup(&self, name: &[u8]) -> Option<EndpointId> {
        self.lookup_count.fetch_add(1, Ordering::Relaxed);
        let hash = fnv1a(name);
        let inner = self.inner.lock();
        inner.lookup(hash).and_then(EndpointId::new)
    }

    /// Removes an endpoint from registry.
    pub fn unregister(&self, name: &[u8]) -> bool {
        let hash = fnv1a(name);
        let mut inner = self.inner.lock();
        inner.remove(hash)
    }

    /// Removes an endpoint from registry by its identifier.
    pub fn unregister_by_endpoint(&self, ep_id: EndpointId) -> bool {
        let mut inner = self.inner.lock();
        inner.remove_by_endpoint(ep_id.get())
    }

    /// Returns number of registered endpoints.
    pub fn count(&self) -> usize {
        let inner = self.inner.lock();
        inner.count()
    }

    /// Lookup statistics.
    pub fn lookup_count(&self) -> u64 {
        self.lookup_count.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Static global registry
// ─────────────────────────────────────────────────────────────────────────────

/// Global endpoint registry.
pub static ENDPOINT_REGISTRY: NamedEndpointRegistry = NamedEndpointRegistry::new();
static ENDPOINT_REGISTRY_READY: AtomicBool = AtomicBool::new(false);

/// Explicitly initializes the global endpoint registry.
///
/// The registry is statically allocated; this function publishes the ready state from
/// `ipc_init()` to prevent implicit initialization in an IPC context.
///
/// # Safety
/// Called once at boot, before any endpoint_create()/lookup().
pub unsafe fn init_endpoint_registry() {
    ENDPOINT_REGISTRY_READY.store(true, Ordering::Release);
}

#[inline]
pub fn endpoint_registry_initialized() -> bool {
    ENDPOINT_REGISTRY_READY.load(Ordering::Acquire)
}

/// Registers an endpoint in the global registry.
pub fn register_endpoint(name: &[u8], ep_id: EndpointId) -> Result<(), IpcError> {
    if !endpoint_registry_initialized() {
        return Err(IpcError::InternalError);
    }
    ENDPOINT_REGISTRY.register(name, ep_id)
}

/// Looks up an endpoint in the global registry.
pub fn lookup_endpoint(name: &[u8]) -> Option<EndpointId> {
    if !endpoint_registry_initialized() {
        return None;
    }
    ENDPOINT_REGISTRY.lookup(name)
}

/// Removes an endpoint from the global registry.
pub fn unregister_endpoint(name: &[u8]) -> bool {
    if !endpoint_registry_initialized() {
        return false;
    }
    ENDPOINT_REGISTRY.unregister(name)
}

/// Removes an endpoint from the global registry by its numeric identifier.
pub fn unregister_endpoint_by_id(ep_id: EndpointId) -> bool {
    if !endpoint_registry_initialized() {
        return false;
    }
    ENDPOINT_REGISTRY.unregister_by_endpoint(ep_id)
}
