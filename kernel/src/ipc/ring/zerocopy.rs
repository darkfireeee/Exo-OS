// kernel/src/ipc/ring/zerocopy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-COPY RING — partage de page physique entre producteur et consommateur
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Pour les grands volumes de données (>= 4 KB), copier les données dans le
// ring serait trop coûteux. Zero-copy partage directement la page physique
// entre les deux espaces d'adressage grâce au flag NO_COW.
//
// FLUX :
//   1. Producteur alloue un buffer depuis le pool SHM
//      (shared_memory/pool.rs : alloc <100 ns).
//   2. Producteur écrit ses données dedans.
//   3. Producteur envoie un ZeroCopyRef (24 bytes) dans le SpscRing normal.
//   4. Consommateur reçoit la ZeroCopyRef, lit les données sans copie.
//   5. Consommateur libère le buffer vers le pool SHM.
//
// SÉCURITÉ :
//   • Le mapping NO_COW interdit au CoW de créer des copies transparentes.
//   • Les droits d'accès sont validés via security::access_control::check_access() avant le mapping.
//   • La page est immutable côté consommateur (R/O mapping).
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
pub use crate::ipc::core::transfer::ZeroCopyRef;
use crate::ipc::core::{IpcError, MsgFlags, array_index_nospec};
use super::spsc::SpscRing;

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyBuffer — buffer SHM avec compteur de références
// ─────────────────────────────────────────────────────────────────────────────

/// Métadonnées d'un buffer zero-copy actif.
/// Stocké dans le pool SHM à côté des données.
#[repr(C, align(8))]
pub struct ZeroCopyBuffer {
    /// Adresse physique du début des données (alignée sur PAGE_SIZE).
    pub phys_addr: u64,
    /// Taille totale du buffer en bytes.
    pub capacity:  u32,
    /// Nombre de bytes écrits (longueur utile).
    pub length:    AtomicU32,
    /// Compteur de références : 1 = producteur seul, 2 = prod + cons, 0 = libre.
    pub refcount:  AtomicU32,
    /// Index dans le pool SHM (pour libération).
    pub pool_idx:  u32,
    /// Génération du slot (détecte les réutilisations UAF).
    pub generation: AtomicU64,
}

impl ZeroCopyBuffer {
    /// Crée un ZeroCopyBuffer non-initialisé (pour usage dans des statics/tables).
    /// refcount = 0 = buffer libre, jamais utilisé sans init préalable.
    pub const fn new_uninit() -> Self {
        Self {
            phys_addr: 0,
            capacity: 0,
            length: AtomicU32::new(0),
            refcount: AtomicU32::new(0),
            pool_idx: 0,
            generation: AtomicU64::new(0),
        }
    }

    /// Initialise un buffer à partir d'une allocation SHM.
    pub fn init(phys_addr: u64, capacity: u32, pool_idx: u32) -> Self {
        Self {
            phys_addr,
            capacity,
            length:     AtomicU32::new(0),
            refcount:   AtomicU32::new(1), // commence à 1 (producteur)
            pool_idx,
            generation: AtomicU64::new(1),
        }
    }

    /// Incrémente le refcount (ajout d'un récepteur).
    #[inline(always)]
    pub fn acquire(&self) {
        self.refcount.fetch_add(1, Ordering::Acquire);
    }

    /// Décrémente le refcount. Retourne true si le buffer peut être libéré.
    #[inline(always)]
    pub fn release(&self) -> bool {
        let prev = self.refcount.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            // Dernier utilisateur — invalider la génération.
            self.generation.fetch_add(1, Ordering::AcqRel);
            return true;
        }
        false
    }

    /// Construit une `ZeroCopyRef` pour envoyer dans le ring.
    #[inline(always)]
    pub fn to_ref(&self) -> ZeroCopyRef {
        ZeroCopyRef::new(
            self.phys_addr,
            self.length.load(Ordering::Acquire),
            self.pool_idx,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyRing — ring SPSC spécialisé pour les ZeroCopyRef
// ─────────────────────────────────────────────────────────────────────────────

/// Ring SPSC pour l'échange de références zero-copy.
///
/// Taille du ring réduite car les transferts zero-copy sont rarement très
/// nombreux simultanément (principalement DMA / streaming vidéo).
const ZC_RING_SIZE: usize = 512;
const ZC_RING_MASK: usize = ZC_RING_SIZE - 1;

/// Un slot du ring zero-copy — contient uniquement une ZeroCopyRef.
#[repr(C, align(32))]
struct ZcSlot {
    seq: AtomicU64,
    zc:  ZeroCopyRef,
    _pad: [u8; 4],
}

impl ZcSlot {
    const fn zeroed() -> Self {
        Self {
            seq:  AtomicU64::new(0),
            zc:   ZeroCopyRef { phys_addr: 0, length: 0, pool_idx: 0 },
            _pad: [0u8; 4],
        }
    }
}

const _: () = assert!(
    core::mem::size_of::<ZcSlot>() == 32,
    "ZcSlot doit faire 32 bytes"
);

/// Ring SPSC pour les échanges zero-copy.
pub struct ZeroCopyRing {
    head:  AtomicU64,
    _pad1: [u8; 56],
    tail:  AtomicU64,
    _pad2: [u8; 56],
    slots: [ZcSlot; ZC_RING_SIZE],
}

// SAFETY: accès contrôlé par head/tail/sequence.
unsafe impl Send for ZeroCopyRing {}
unsafe impl Sync for ZeroCopyRing {}

impl ZeroCopyRing {
    /// Crée un ring zéro-copie.
    pub const fn new() -> Self {
        const ZERO_SLOT: ZcSlot = ZcSlot::zeroed();
        Self {
            head:  AtomicU64::new(0),
            _pad1: [0u8; 56],
            tail:  AtomicU64::new(0),
            _pad2: [0u8; 56],
            slots: [ZERO_SLOT; ZC_RING_SIZE],
        }
    }

    /// Initialise les séquences.
    pub fn init(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            slot.seq.store(i as u64, Ordering::Relaxed);
        }
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Accès Spectre-safe (RÈGLE IPC-08).
    #[inline(always)]
    fn slot_at(&self, pos: u64) -> &ZcSlot {
        let idx = array_index_nospec((pos as usize) & ZC_RING_MASK, ZC_RING_SIZE);
        &self.slots[idx]
    }

    /// Envoie une référence zero-copy.
    pub fn push(&self, zc: ZeroCopyRef) -> Result<(), IpcError> {
        let pos = self.head.load(Ordering::Relaxed);
        let slot = self.slot_at(pos);
        if slot.seq.load(Ordering::Acquire) != pos {
            return Err(IpcError::WouldBlock);
        }
        // SAFETY: SPSC — seul producteur. addr_of! évite de créer une &T intermédiaire,
        // ce qui éviterait l'UB lié au strict-aliasing.
        // SAFETY: addr_of! évite UB strict-aliasing ; array_index_nospec (IPC-08).
        unsafe {
            let safe_idx = array_index_nospec((pos as usize) & ZC_RING_MASK, ZC_RING_SIZE);
            let s = core::ptr::addr_of!(self.slots[safe_idx]);
            core::ptr::write(
                core::ptr::addr_of!((*s).zc) as *mut ZeroCopyRef,
                zc,
            );
        }
        slot.seq.store(pos + 1, Ordering::Release);
        self.head.store(pos + 1, Ordering::Relaxed);
        Ok(())
    }

    /// Reçoit une référence zero-copy.
    pub fn pop(&self) -> Result<ZeroCopyRef, IpcError> {
        let pos  = self.tail.load(Ordering::Relaxed);
        let slot = self.slot_at(pos);
        if slot.seq.load(Ordering::Acquire) != pos + 1 {
            return Err(IpcError::WouldBlock);
        }
        // SAFETY: séquence validée → données disponibles. SPSC → seul consommateur.
        let zc = unsafe {
            let safe_idx = array_index_nospec((pos as usize) & ZC_RING_MASK, ZC_RING_SIZE);
            core::ptr::read(&self.slots[safe_idx].zc) // IPC-08: safe_idx borne par nospec
        };
        slot.seq.store(pos + ZC_RING_SIZE as u64, Ordering::Release);
        self.tail.store(pos + 1, Ordering::Relaxed);
        Ok(zc)
    }

    /// Retourne vrai si le ring est vide.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        let pos  = self.tail.load(Ordering::Relaxed);
        self.slot_at(pos).seq.load(Ordering::Relaxed) != pos + 1
    }
}
