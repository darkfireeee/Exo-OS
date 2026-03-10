// kernel/src/ipc/endpoint/lifecycle.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// LIFECYCLE — Création, destruction, cleanup des endpoints
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Gère le cycle de vie complet d'un endpoint :
//   CREATE → LISTEN → [accept/connect cycles...] → DRAIN → CLOSE → DESTROY
//
// CLEANUP :
//   À la fermeture, toutes les connexions en cours reçoivent un message
//   CHANNEL_CLOSED avant que l'endpoint soit marqué Destroyed.
//   Les threads en attente dans le backlog reçoivent ConnRefused.
//
// RÈGLE : l'endpoint n'est jamais libéré tant qu'active_conns > 0.
//         Une période de drain est obligatoire.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU64, Ordering};
use crate::ipc::core::types::{EndpointId, IpcError, alloc_endpoint_id};
use crate::ipc::core::constants::MAX_ENDPOINTS;
use crate::scheduler::core::task::ThreadId;
use crate::scheduler::sync::spinlock::SpinLock;
use super::descriptor::{EndpointDesc, EndpointName, EndpointState};
use super::registry::{register_endpoint, unregister_endpoint};

// ─────────────────────────────────────────────────────────────────────────────
// EndpointPool — pool statique d'EndpointDesc
// ─────────────────────────────────────────────────────────────────────────────

/// État d'un slot dans le pool.
#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)]
enum SlotState {
    Free  = 0,
    InUse = 1,
}

/// Pool statique d'endpoints.
///
/// Alloue MAX_ENDPOINTS descripteurs d'endpoints sans heap.
/// L'allocation est protégée par un SpinLock sur le bitmap.
struct EndpointPool {
    /// Bitmap des slots libres / occupés (MAX_ENDPOINTS / 64 mots).
    bitmap: [AtomicU64; MAX_ENDPOINTS / 64],
    /// Nombre d'endpoints actifs.
    active: AtomicU64,
}

impl EndpointPool {
    const fn new() -> Self {
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            bitmap: [ZERO; MAX_ENDPOINTS / 64],
            active: AtomicU64::new(0),
        }
    }

    /// Alloue un slot libre. Retourne l'index ou None si pool épuisé.
    fn alloc(&self) -> Option<usize> {
        for (word_idx, word) in self.bitmap.iter().enumerate() {
            let bits = word.load(Ordering::Relaxed);
            if bits == u64::MAX {
                continue; // tous occupés
            }
            // Trouver le premier bit à 0.
            let bit = (!bits).trailing_zeros() as usize;
            let mask = 1u64 << bit;
            // CAS pour réserver le slot.
            if word
                .compare_exchange_weak(bits, bits | mask, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.active.fetch_add(1, Ordering::Relaxed);
                return Some(word_idx * 64 + bit);
            }
        }
        None
    }

    /// Libère un slot.
    fn free(&self, idx: usize) {
        debug_assert!(idx < MAX_ENDPOINTS, "idx hors bornes");
        let word = idx / 64;
        let bit  = idx % 64;
        let mask = 1u64 << bit;
        self.bitmap[word].fetch_and(!mask, Ordering::Release);
        self.active.fetch_sub(1, Ordering::Relaxed);
    }

    fn active_count(&self) -> u64 {
        self.active.load(Ordering::Relaxed)
    }
}

// Pool global.
static EP_POOL_BITMAP: EndpointPool = EndpointPool::new();

// Tableau des descripteurs — accès via index obtenu depuis EndpointPool.
// SAFETY: accès protégé par le protocole bitmap + SpinLock par descripteur.
static EP_POOL_DESCS: SpinLock<[Option<EndpointDescBox>; MAX_ENDPOINTS]> =
    SpinLock::new([const { None }; MAX_ENDPOINTS]);

/// Wrapper newtype pour rendre EndpointDesc Option-able statiquement.
struct EndpointDescBox(core::mem::MaybeUninit<EndpointDesc>);

impl EndpointDescBox {
    fn init(desc: EndpointDesc) -> Self {
        Self(core::mem::MaybeUninit::new(desc))
    }
    /// # Safety : appelé uniquement quand le slot est In Use.
    unsafe fn as_ref(&self) -> &EndpointDesc {
        self.0.assume_init_ref()
    }
    /// # Safety : appelé uniquement quand le slot est In Use.
    #[allow(dead_code)]
    unsafe fn as_mut(&mut self) -> &mut EndpointDesc {
        self.0.assume_init_mut()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique du lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Crée et enregistre un nouvel endpoint.
///
/// # Arguments
/// - `name`  : nom ASCII de l'endpoint (max 64 chars).
/// - `owner` : thread propriétaire initial.
///
/// # Retour
/// `Ok(EndpointId)` — l'identifiant unique de l'endpoint créé.
pub fn endpoint_create(name: &[u8], owner: ThreadId) -> Result<EndpointId, IpcError> {
    let ep_name = EndpointName::from_bytes(name)?;
    let ep_id   = alloc_endpoint_id();

    let idx = EP_POOL_BITMAP.alloc().ok_or(IpcError::ResourceExhausted)?;
    let desc = EndpointDesc::new(ep_id, ep_name, owner);

    {
        let mut pool = EP_POOL_DESCS.lock();
        pool[idx]    = Some(EndpointDescBox::init(desc));
    }

    // Enregistrement dans le registre nom → id.
    if let Err(e) = register_endpoint(name, ep_id) {
        // Rollback : libérer le slot.
        {
            let mut pool = EP_POOL_DESCS.lock();
            pool[idx]    = None;
        }
        EP_POOL_BITMAP.free(idx);
        return Err(e);
    }

    Ok(ep_id)
}

/// Met l'endpoint en écoute (transition Created → Listening).
pub fn endpoint_listen(ep_id: EndpointId) -> Result<(), IpcError> {
    with_endpoint(ep_id, |desc| desc.start_listen())
}

/// Initie la fermeture d'un endpoint (transition → Draining).
///
/// Les connexions existantes continueront jusqu'à leur fermeture naturelle.
/// Les nouvelles connexions sont refusées.
pub fn endpoint_close(ep_id: EndpointId) -> Result<(), IpcError> {
    with_endpoint(ep_id, |desc| {
        desc.state.store(EndpointState::Draining as u32, Ordering::Release);
        Ok(())
    })
}

/// Détruit un endpoint (uniquement si active_conns == 0).
pub fn endpoint_destroy(name: &[u8], ep_id: EndpointId) -> Result<(), IpcError> {
    // Vérifier qu'il n'y a plus de connexions actives.
    let active = {
        let pool = EP_POOL_DESCS.lock();
        // Trouver l'index par ep_id.
        pool.iter()
            .position(|slot| {
                if let Some(ref b) = slot {
                    // SAFETY: accessed under lock, slot is Some.
                    unsafe { b.as_ref().id == ep_id }
                } else {
                    false
                }
            })
            .map(|idx| {
                // SAFETY: slot is Some.
                unsafe { pool[idx].as_ref().unwrap().as_ref().active_conns.load(Ordering::Acquire) }
            })
    };

    if active == Some(0) || active.is_none() {
        // Retirer du registre + libérer le slot.
        unregister_endpoint(name);
        // Trouver et libérer le slot dans le pool.
        let mut pool = EP_POOL_DESCS.lock();
        for (idx, slot) in pool.iter_mut().enumerate() {
            let matches = if let Some(ref b) = slot {
                // SAFETY: accessed under lock.
                unsafe { b.as_ref().id == ep_id }
            } else {
                false
            };
            if matches {
                *slot = None;
                drop(pool); // libérer le lock avant de modifier le bitmap
                EP_POOL_BITMAP.free(idx);
                return Ok(());
            }
        }
    }

    Err(IpcError::WouldBlock) // encore des connexions actives
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper : accès à un endpoint par son id
// ─────────────────────────────────────────────────────────────────────────────

/// Exécute une closure sur le descripteur d'un endpoint identifié par son id.
/// Retourne `Err(EndpointNotFound)` si l'id est inconnu.
fn with_endpoint<F, R>(ep_id: EndpointId, f: F) -> Result<R, IpcError>
where
    F: FnOnce(&EndpointDesc) -> Result<R, IpcError>,
{
    let pool = EP_POOL_DESCS.lock();
    for slot in pool.iter() {
        if let Some(ref b) = slot {
            // SAFETY: accessed under lock.
            let desc = unsafe { b.as_ref() };
            if desc.id == ep_id {
                return f(desc);
            }
        }
    }
    Err(IpcError::EndpointNotFound)
}

/// Retourne le nombre d'endpoints actifs dans le pool.
pub fn active_endpoint_count() -> u64 {
    EP_POOL_BITMAP.active_count()
}
