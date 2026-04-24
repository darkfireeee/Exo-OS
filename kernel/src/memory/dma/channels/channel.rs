// kernel/src/memory/dma/channels/channel.rs
//
// Ring buffer producer/consumer pour canaux DMA individuels.
//
// Ce module implémente le mécanisme bas niveau d'un canal DMA :
//   - Le producteur (driver) dépose des transactions via `submit()`.
//   - Le consommateur (moteur DMA / IRQ handler) retire via `consume()`.
//   - Ring buffer lock-free single-producer/single-consumer (SPSC).
//   - Backpressure : `submit()` retourne Err si le ring est plein.
//
// COUCHE 0 — aucune dépendance externe.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaChannelId, DmaDirection, DmaError, DmaMapFlags, DmaTransactionId,
};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Profondeur du ring buffer d'un canal (power of 2 pour masque rapide).
pub const CHANNEL_RING_SIZE: usize = 128;
const RING_MASK: usize = CHANNEL_RING_SIZE - 1;

// ─────────────────────────────────────────────────────────────────────────────
// COMMANDE DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Une commande de transfert dans le ring buffer.
///
/// Format compact (64 bytes) pour tenir dans une cache line.
#[repr(C, align(64))]
#[derive(Copy, Clone)]
pub struct DmaCommand {
    /// Transaction parente (pour signalement de completion/erreur).
    pub txn_id: DmaTransactionId,
    /// Adresse physique source.
    pub src_phys: PhysAddr,
    /// Adresse physique de destination.
    pub dst_phys: PhysAddr,
    /// Longueur du transfert en octets.
    pub len: u32,
    /// Direction du transfert.
    pub direction: u8, // DmaDirection as u8
    /// Flags de mapping DMA.
    pub flags: u32, // DmaMapFlags.0
    /// Padding pour exactement 64 bytes.
    _pad: [u8; 7],
}

const _: () = assert!(
    core::mem::size_of::<DmaCommand>() == 64,
    "DmaCommand doit faire exactement 64 bytes"
);

impl DmaCommand {
    pub const EMPTY: Self = DmaCommand {
        txn_id: DmaTransactionId::INVALID,
        src_phys: PhysAddr::new(0),
        dst_phys: PhysAddr::new(0),
        len: 0,
        direction: 0,
        flags: 0,
        _pad: [0u8; 7],
    };

    pub fn new(
        txn_id: DmaTransactionId,
        src_phys: PhysAddr,
        dst_phys: PhysAddr,
        len: u32,
        direction: DmaDirection,
        flags: DmaMapFlags,
    ) -> Self {
        DmaCommand {
            txn_id,
            src_phys,
            dst_phys,
            len,
            direction: direction as u8,
            flags: flags.0,
            _pad: [0u8; 7],
        }
    }

    pub fn is_valid(self) -> bool {
        self.txn_id.0 != DmaTransactionId::INVALID.0 && self.len > 0
    }

    pub fn direction(self) -> DmaDirection {
        match self.direction {
            0 => DmaDirection::ToDevice,
            1 => DmaDirection::FromDevice,
            2 => DmaDirection::Bidirection,
            _ => DmaDirection::None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RING BUFFER SPSC
// ─────────────────────────────────────────────────────────────────────────────

/// Ring buffer SPSC (Single-Producer/Single-Consumer) pour commandes DMA.
///
/// ## Garanties
/// - `submit()` peut être appelé depuis n'importe quel contexte (même IRQ)
///   par **un seul** producteur à la fois.
/// - `consume()` est appelé uniquement par le moteur DMA ou le handler IRQ,
///   par **un seul** consommateur à la fois.
/// - Aucun verrou requis pour les opérations normales (lock-free SPSC).
///
/// ## Débordement
/// Si le ring est plein, `submit()` retourne `Err(DmaError::OutOfMemory)`.
/// Le producteur est responsable de la gestion de la backpressure.
pub struct DmaChannelRing {
    /// Canal propriétaire de ce ring.
    pub channel_id: DmaChannelId,
    /// Commandes dans le ring.
    /// UnsafeCell pour la mutation SPSC sans verrou.
    ring: UnsafeCell<[DmaCommand; CHANNEL_RING_SIZE]>,
    /// Index du prochain slot d'écriture (producteur).
    head: AtomicU32,
    /// Index du prochain slot de lecture (consommateur).
    tail: AtomicU32,
    /// Ring actif (peut recevoir des soumissions).
    active: AtomicBool,
    /// Statistiques.
    pub submitted: AtomicU64,
    pub consumed: AtomicU64,
    pub dropped_full: AtomicU64,
    pub errors_injected: AtomicU64,
}

// SAFETY : SPSC garanti par convention. head modifié uniquement par le producteur,
// tail uniquement par le consommateur. Pas d'accès concurrent aux mêmes slots.
unsafe impl Sync for DmaChannelRing {}
unsafe impl Send for DmaChannelRing {}

impl DmaChannelRing {
    pub const fn new(channel_id: DmaChannelId) -> Self {
        DmaChannelRing {
            channel_id,
            ring: UnsafeCell::new([DmaCommand::EMPTY; CHANNEL_RING_SIZE]),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            active: AtomicBool::new(false),
            submitted: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            dropped_full: AtomicU64::new(0),
            errors_injected: AtomicU64::new(0),
        }
    }

    /// Active ce ring (autorise les soumissions).
    pub fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }

    /// Désactive ce ring (rejette les nouvelles soumissions).
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    // ── Producteur ───────────────────────────────────────────────────────────

    /// Soumet une commande DMA dans le ring.
    ///
    /// # Erreurs
    /// - `DmaError::NotInitialized` : ring inactif.
    /// - `DmaError::OutOfMemory` : ring plein (backpressure).
    /// - `DmaError::InvalidParams` : commande non valide.
    pub fn submit(&self, cmd: DmaCommand) -> Result<(), DmaError> {
        if !self.is_active() {
            return Err(DmaError::NotInitialized);
        }
        if !cmd.is_valid() {
            return Err(DmaError::InvalidParams);
        }

        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        // Ring plein : head - tail >= CHANNEL_RING_SIZE.
        if (head.wrapping_sub(tail)) as usize >= CHANNEL_RING_SIZE {
            self.dropped_full.fetch_add(1, Ordering::Relaxed);
            return Err(DmaError::OutOfMemory);
        }

        let slot = (head as usize) & RING_MASK;
        // SAFETY: head % SIZE ∈ [0,SIZE); pas de consommateur concurrent (tail != head+1 vérifié).
        unsafe {
            (*self.ring.get())[slot] = cmd;
        }

        // Release fence : The slot write must be visible to the consumer before
        // the head increment.
        self.head.store(head.wrapping_add(1), Ordering::Release);
        self.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    // ── Consommateur ─────────────────────────────────────────────────────────

    /// Retire la prochaine commande du ring.
    ///
    /// Retourne `None` si le ring est vide.
    /// Appelé depuis le contexte IRQ / moteur DMA.
    pub fn consume(&self) -> Option<DmaCommand> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None; // Ring vide.
        }

        let slot = (tail as usize) & RING_MASK;
        // SAFETY: tail < head, producteur n'écrit plus à `tail`.
        let cmd = unsafe { (*self.ring.get())[slot] };

        // SAFETY: slot nettoyé visible avant incrément tail (Release); consommateur unique.
        unsafe {
            (*self.ring.get())[slot] = DmaCommand::EMPTY;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        self.consumed.fetch_add(1, Ordering::Relaxed);
        Some(cmd)
    }

    /// Consomme jusqu'à `max` commandes en une passe.
    /// Retourne le slice des commandes consommées dans `out_buf`.
    /// Retourne le nombre de commandes écrites dans `out_buf`.
    pub fn consume_batch(&self, out_buf: &mut [DmaCommand]) -> usize {
        let mut count = 0;
        while count < out_buf.len() {
            match self.consume() {
                Some(cmd) => {
                    out_buf[count] = cmd;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    // ── Observateurs ─────────────────────────────────────────────────────────

    /// Nombre approximatif de commandes en attente dans le ring.
    pub fn pending(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        (head.wrapping_sub(tail)) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }
    pub fn is_full(&self) -> bool {
        self.pending() >= CHANNEL_RING_SIZE
    }
    pub fn capacity(&self) -> usize {
        CHANNEL_RING_SIZE
    }
}
