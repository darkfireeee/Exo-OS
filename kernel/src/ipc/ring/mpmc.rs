// kernel/src/ipc/ring/mpmc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MPMC RING — Multi-Producer Multi-Consumer lock-free (séquence atomique)
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Algorithme de Dmitry Vyukov (adapté) :
//   • Chaque cellule possède son propre numéro de séquence atomique.
//   • Producteur : CAS sur head → réserve une position.
//   • Consommateur : CAS sur tail → réserve une position.
//   • Aucun lock global — chaque thread progresse indépendamment.
//
// GARANTIES :
//   • Wait-free pour les threads qui réussissent leur CAS du premier coup.
//   • Lock-free dans le cas général (au moins un thread progresse).
//   • Ordre FIFO approximatif (pas garanti si plusieurs producteurs simultanés).
//
// USAGE : Canal MPMC pour event dispatching, worker pools.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::ipc::core::{IpcError, MsgFlags, alloc_message_id, RING_SIZE, RING_MASK, array_index_nospec};
use crate::ipc::core::transfer::{MessageHeader, RingSlot};
use super::slot::SlotCell;

/// Nombre de slots dans un ring MPMC (= RING_SIZE).
/// Re-exporté pour usage dans channel/mpmc.rs.
pub const MPMC_RING_SIZE: usize = RING_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// MpmcRing
// ─────────────────────────────────────────────────────────────────────────────

/// Ring MPMC lock-free.
/// Head et tail sont chacun sur leur propre cache line.
#[repr(C, align(64))]
pub struct MpmcRing {
    /// Curseur producteur — partagé entre plusieurs threads.
    head: [u8; 64],           // AtomicU64 à offset 0 dans ce cache pad
    /// Curseur consommateur — partagé entre plusieurs threads.
    tail: [u8; 64],
    /// Tableau des cellules.
    cells: UnsafeCell<[SlotCell; RING_SIZE]>,
}

// SAFETY: MpmcRing est thread-safe par construction (atomics + séquences).
unsafe impl Send for MpmcRing {}
unsafe impl Sync for MpmcRing {}

impl MpmcRing {
    /// Retourne le curseur head (producteur) comme AtomicU64.
    #[inline(always)]
    fn head_atomic(&self) -> &AtomicU64 {
        // SAFETY: head[0..8] est aligné sur 64 bytes et contient un AtomicU64.
        unsafe { &*(self.head.as_ptr() as *const AtomicU64) }
    }

    /// Retourne le curseur tail (consommateur) comme AtomicU64.
    #[inline(always)]
    fn tail_atomic(&self) -> &AtomicU64 {
        // SAFETY: tail[0..8] est aligné sur 64 bytes et contient un AtomicU64.
        unsafe { &*(self.tail.as_ptr() as *const AtomicU64) }
    }

    /// Crée un ring non initialisé.
    ///
    /// # Safety
    /// `init()` DOIT être appelé avant toute utilisation.
    pub const fn new_uninit() -> Self {
        const INIT_CELL: SlotCell = SlotCell::new_at(0);
        Self {
            head:  [0u8; 64],
            tail:  [0u8; 64],
            cells: UnsafeCell::new([INIT_CELL; RING_SIZE]),
        }
    }

    /// Initialise le ring — doit être appelé une seule fois après construction.
    pub fn init(&self) {
        self.head_atomic().store(0, Ordering::Relaxed);
        self.tail_atomic().store(0, Ordering::Relaxed);
        let cells = unsafe { &mut *self.cells.get() };
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.sequence.store(i as u64, Ordering::Relaxed);
        }
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Accède à la cellule à la position `pos`.
    /// Utilise array_index_nospec (RÈGLE IPC-08 — Spectre v1).
    #[inline(always)]
    fn cell_at(&self, pos: u64) -> &SlotCell {
        let cells = unsafe { &*self.cells.get() };
        let idx = array_index_nospec((pos as usize) & RING_MASK, RING_SIZE);
        &cells[idx]
    }

    // ───────────────────────────── PUSH (producteur) ─────────────────────

    /// Envoie un message (copie).
    ///
    /// # Sûreté multi-thread
    /// Peut être appelé depuis plusieurs threads simultanément.
    pub fn push_copy(&self, src: &[u8], flags: MsgFlags) -> Result<u64, IpcError> {
        if src.len() > crate::ipc::core::MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        loop {
            let pos = self.head_atomic().fetch_add(1, Ordering::AcqRel);
            let cell = self.cell_at(pos);

            // Attendre que la séquence de la cellule == pos (libre).
            let mut spin = 0u32;
            loop {
                let seq = cell.load_seq();
                let diff = (seq as i64).wrapping_sub(pos as i64);
                if diff == 0 {
                    break; // slot libre → on peut écrire
                }
                if diff < 0 {
                    // Ring plein — le head a dépassé le tail d'un tour.
                    return Err(IpcError::QueueFull);
                }
                // diff > 0 : un autre producteur a pris ce slot avant nous (rare).
                spin += 1;
                if spin > 1_000 {
                    return Err(IpcError::QueueFull);
                }
                core::hint::spin_loop();
            }

            let id = alloc_message_id();
            // SAFETY: nous avons réservé ce slot via le protocole séquentiel.
            unsafe {
                let slot = (*cell.slot.get()).assume_init_mut();
                slot.header = MessageHeader::new_inline(id, flags, src.len());
                if !src.is_empty() {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr(),
                        slot.payload.as_mut_ptr(),
                        src.len(),
                    );
                }
            }
            // Rendre visible : séquence = pos + 1.
            cell.store_seq(pos + 1);
            return Ok(id.get());
        }
    }

    // ───────────────────────────── POP (consommateur) ────────────────────

    /// Reçoit un message dans `dst`.
    pub fn pop_into(&self, dst: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        loop {
            let pos  = self.tail_atomic().fetch_add(1, Ordering::AcqRel);
            let cell = self.cell_at(pos);

            let mut spin = 0u32;
            loop {
                let seq = cell.load_seq();
                let diff = (seq as i64).wrapping_sub((pos + 1) as i64);
                if diff < 0 {
                    // Ring vide.
                    return Err(IpcError::QueueEmpty);
                }
                if diff == 0 {
                    break; // données disponibles
                }
                spin += 1;
                if spin > 1_000 {
                    return Err(IpcError::QueueEmpty);
                }
                core::hint::spin_loop();
            }

            let (len, msg_flags) = unsafe {
                let slot  = (*cell.slot.get()).assume_init_ref();
                let ln    = slot.header.len as usize;
                let flags = slot.header.flags;
                if ln > dst.len() {
                    cell.store_seq(pos + RING_SIZE as u64);
                    return Err(IpcError::MessageTooLarge);
                }
                if ln > 0 {
                    core::ptr::copy_nonoverlapping(
                        slot.payload.as_ptr(),
                        dst.as_mut_ptr(),
                        ln,
                    );
                }
                (ln, MsgFlags(flags))
            };

            // Libère le slot pour le prochain producteur.
            cell.store_seq(pos + RING_SIZE as u64);
            return Ok((len, msg_flags));
        }
    }

    /// Retourne vrai si le ring est approximativement vide.
    #[inline(always)]
    pub fn is_empty_approx(&self) -> bool {
        let h = self.head_atomic().load(Ordering::Relaxed);
        let t = self.tail_atomic().load(Ordering::Relaxed);
        h == t
    }

    /// Nombre approximatif de messages en attente.
    #[inline(always)]
    pub fn len_approx(&self) -> usize {
        let h = self.head_atomic().load(Ordering::Relaxed);
        let t = self.tail_atomic().load(Ordering::Relaxed);
        h.wrapping_sub(t) as usize
    }
}
