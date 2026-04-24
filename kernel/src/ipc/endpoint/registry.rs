// kernel/src/ipc/endpoint/registry.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ENDPOINT REGISTRY — Registre nom → endpoint (table de hachage statique)
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le registre maintient une table de hachage ouverte (open addressing,
// Robin Hood hashing) des endpoints actifs, indexée par nom.
//
// IMPLÉMENTATION :
//   • Table de hachage statique (MAX_ENDPOINTS entrées).
//   • Hash FNV-1a 64 bits (rapide, sans allocation).
//   • Robin Hood hashing pour minimiser la variance des probes.
//   • Protection par SpinLock (registre partagé entre tous les threads).
//   • Lookup O(1) en moyenne, O(n) au pire (très improbable avec FNV-1a).
//
// RÈGLE ZONE NO-ALLOC : zéro Vec/Box/Arc dans ce fichier.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::ipc::core::constants::MAX_ENDPOINTS;
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Hash FNV-1a 64 bits
// ─────────────────────────────────────────────────────────────────────────────

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Hash FNV-1a d'un slice de bytes.
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
// RegistryEntry — une entrée dans la table de hachage
// ─────────────────────────────────────────────────────────────────────────────

/// Distance de déplacement Robin Hood (0 = slot libre/tombstone).
type RobinHoodDist = u16;

/// Entrée de la table de hachage.
struct RegistryEntry {
    /// Hash du nom (-1 si slot libre, -2 = tombstone).
    hash: u64,
    /// EndpointId stocké.
    ep_id: u64,
    /// Distance Robin Hood depuis la position idéale.
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
// EndpointRegistry — table globale
// ─────────────────────────────────────────────────────────────────────────────

/// Table de hachage des endpoints actifs.
struct RegistryInner {
    /// Slots de la table (open addressing, Robin Hood).
    slots: [RegistryEntry; MAX_ENDPOINTS],
    /// Nombre d'entrées occupées.
    count: usize,
}

impl RegistryInner {
    const fn new() -> Self {
        // SAFETY: RegistryEntry::empty() est une constante valide.
        const ZERO: RegistryEntry = RegistryEntry::empty();
        Self {
            slots: [ZERO; MAX_ENDPOINTS],
            count: 0,
        }
    }

    /// Insère ou met à jour un endpoint.
    fn insert(&mut self, hash: u64, ep_id: u64) -> Result<(), IpcError> {
        if self.count >= MAX_ENDPOINTS * 3 / 4 {
            // Table à 75% — refuser pour éviter trop de collisions.
            return Err(IpcError::ResourceExhausted);
        }
        let mut pos = (hash as usize) % MAX_ENDPOINTS;
        let dist: RobinHoodDist = 0;
        let mut cur_hash = hash;
        let mut cur_ep_id = ep_id;
        let mut cur_dist = dist;

        loop {
            let slot = &mut self.slots[pos];
            if slot.is_empty() || slot.is_tombstone() {
                *slot = RegistryEntry {
                    hash: cur_hash,
                    ep_id: cur_ep_id,
                    rh_dist: cur_dist,
                    _pad: [0u8; 6],
                };
                self.count += 1;
                return Ok(());
            }
            // Robin Hood : si le slot existant est moins loin de sa position idéale,
            // on le déplace.
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

    /// Recherche un endpoint par hash.
    /// Retourne l'EndpointId ou None si introuvable.
    fn lookup(&self, hash: u64) -> Option<u64> {
        let mut pos = (hash as usize) % MAX_ENDPOINTS;
        let mut dist = 0usize;
        loop {
            let slot = &self.slots[pos];
            if slot.is_empty() {
                return None;
            }
            if slot.is_tombstone() {
                pos = (pos + 1) % MAX_ENDPOINTS;
                dist += 1;
                continue;
            }
            if slot.hash == hash {
                return Some(slot.ep_id);
            }
            // Robin Hood : si la distance courante dépasse celle du slot, l'élément
            // cherché ne peut pas être là (sous-invariant Robin Hood).
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

    /// Supprime un endpoint du registre.
    fn remove(&mut self, hash: u64) -> bool {
        let mut pos = (hash as usize) % MAX_ENDPOINTS;
        let mut dist = 0usize;
        loop {
            let slot = &mut self.slots[pos];
            if slot.is_empty() {
                return false;
            }
            if !slot.is_tombstone() && slot.hash == hash {
                slot.hash = RegistryEntry::TOMBSTONE;
                slot.ep_id = 0;
                slot.rh_dist = 0;
                self.count -= 1;
                return true;
            }
            pos = (pos + 1) % MAX_ENDPOINTS;
            dist += 1;
            if dist >= MAX_ENDPOINTS {
                return false;
            }
        }
    }

    /// Retourne le nombre d'endpoints enregistrés.
    #[inline(always)]
    fn count(&self) -> usize {
        self.count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NamedEndpointRegistry — registre global, thread-safe
// ─────────────────────────────────────────────────────────────────────────────

/// Registre global des endpoints IPC.
///
/// Thread-safe via SpinLock.
/// Lookup via FNV-1a hash sur le nom ASCII.
pub struct NamedEndpointRegistry {
    inner: SpinLock<RegistryInner>,
    /// Compteur total de lookups (instrumentation).
    lookup_count: AtomicU64,
    /// Compteur total d'insertions.
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

    /// Enregistre un endpoint par son nom.
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

    /// Recherche un endpoint par son nom.
    pub fn lookup(&self, name: &[u8]) -> Option<EndpointId> {
        self.lookup_count.fetch_add(1, Ordering::Relaxed);
        let hash = fnv1a(name);
        let inner = self.inner.lock();
        inner.lookup(hash).and_then(EndpointId::new)
    }

    /// Retire un endpoint du registre.
    pub fn unregister(&self, name: &[u8]) -> bool {
        let hash = fnv1a(name);
        let mut inner = self.inner.lock();
        inner.remove(hash)
    }

    /// Retourne le nombre d'endpoints enregistrés.
    pub fn count(&self) -> usize {
        let inner = self.inner.lock();
        inner.count()
    }

    /// Statistiques de lookup.
    pub fn lookup_count(&self) -> u64 {
        self.lookup_count.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registre global statique
// ─────────────────────────────────────────────────────────────────────────────

/// Registre global des endpoints.
pub static ENDPOINT_REGISTRY: NamedEndpointRegistry = NamedEndpointRegistry::new();

/// Enregistre un endpoint dans le registre global.
pub fn register_endpoint(name: &[u8], ep_id: EndpointId) -> Result<(), IpcError> {
    ENDPOINT_REGISTRY.register(name, ep_id)
}

/// Recherche un endpoint dans le registre global.
pub fn lookup_endpoint(name: &[u8]) -> Option<EndpointId> {
    ENDPOINT_REGISTRY.lookup(name)
}

/// Retire un endpoint du registre global.
pub fn unregister_endpoint(name: &[u8]) -> bool {
    ENDPOINT_REGISTRY.unregister(name)
}
