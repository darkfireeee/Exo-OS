// kernel/src/ipc/ring/slot.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SLOT MANAGEMENT — gestion des slots dans les rings IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un slot est l'unité atomique de communication dans un ring.
// Chaque slot possède un état (Sequence State) qui contrôle qui peut
// lire ou écrire dedans, sans lock.
//
// TECHNIQUE : Sequence-based ring (Dmitry Vyukov MPMC Queue).
// L'état d'un slot est encodé dans un AtomicU64 qui contient le numéro
// de séquence du slot. Le producteur attend seq == pos, le consommateur
// attend seq == pos + 1.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::ipc::core::constants::RING_SIZE;
use crate::ipc::core::transfer::RingSlot;
use crate::ipc::core::types::array_index_nospec;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SlotCell — cellule atomique d'un ring
// ─────────────────────────────────────────────────────────────────────────────

/// Une cellule du ring : numéro de séquence + données.
/// Le numéro de séquence encode l'état de la cellule :
///   - seq == index          → libre (prêt pour un producteur)
///   - seq == index + 1      → données disponibles (prêt pour un consommateur)
///   - autre                 → en cours d'écriture ou de lecture
#[repr(C, align(64))]
pub struct SlotCell {
    /// Numéro de séquence atomique — détermine qui peut accéder au slot.
    pub sequence: AtomicU64,
    /// Données inlinées dans le slot.
    pub slot: UnsafeCell<MaybeUninit<RingSlot>>,
    /// Padding pour éviter la false sharing (compléter à 64 bytes minimum).
    _pad: [u8; 0],
}

// SAFETY: SlotCell est accédée uniquement via les atomics et les accords
// de séquence. L'UnsafeCell est protégée par le protocole séquentiel.
unsafe impl Send for SlotCell {}
unsafe impl Sync for SlotCell {}

impl SlotCell {
    /// Initialise une cellule avec son index (séquence initiale = index).
    pub const fn new_at(index: u64) -> Self {
        Self {
            sequence: AtomicU64::new(index),
            slot: UnsafeCell::new(MaybeUninit::uninit()),
            _pad: [],
        }
    }

    /// Charge le numéro de séquence (Acquire).
    #[inline(always)]
    pub fn load_seq(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// Stocke le numéro de séquence (Release).
    #[inline(always)]
    pub fn store_seq(&self, seq: u64) {
        self.sequence.store(seq, Ordering::Release)
    }

    /// Accède au slot de données de manière exclusive (précondition : séquence acquise).
    ///
    /// # Safety
    /// L'appelant doit posséder l'accès exclusif au slot via le protocole de séquence.
    #[inline(always)]
    pub unsafe fn data_mut(&self) -> &mut MaybeUninit<RingSlot> {
        &mut *self.slot.get()
    }

    /// Accède au slot de données en lecture (précondition : séquence acquise).
    ///
    /// # Safety
    /// L'appelant doit posséder l'accès en lecture via le protocole de séquence.
    #[inline(always)]
    pub unsafe fn data_ref(&self) -> &MaybeUninit<RingSlot> {
        &*self.slot.get()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RingBuffer — tableau statique de RING_SIZE SlotCells
// ─────────────────────────────────────────────────────────────────────────────

/// Buffer ring de taille fixe RING_SIZE.
/// Alloué statiquement ou embarqué dans une structure plus grande.
///
/// NOTE : taille totale ≈ RING_SIZE × sizeof(SlotCell)
///       = 4096 × 4112 bytes ≈ 16 MiB par ring.
/// En pratique, les rings sont créés dynamiquement depuis le pool SHM
/// plutôt qu'alloués statiquement tous. Un seul ring static = ring système.
pub struct RingBuffer {
    cells: [SlotCell; RING_SIZE],
}

impl RingBuffer {
    /// Initialise un ring buffer (séquences = index pour chaque cellule).
    pub fn init(&mut self) {
        for (i, cell) in self.cells.iter().enumerate() {
            cell.sequence.store(i as u64, Ordering::Relaxed);
        }
        // Barrière finale pour que tous les stores soient visibles.
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Accède à la cellule à l'index `pos % RING_SIZE`.
    /// Utilise array_index_nospec (RÈGLE IPC-08 — Spectre v1).
    #[inline(always)]
    pub fn cell_at(&self, pos: u64) -> &SlotCell {
        let idx = array_index_nospec((pos as usize) & (RING_SIZE - 1), RING_SIZE);
        &self.cells[idx]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SlotGuard — RAII pour la libération automatique des slots
// ─────────────────────────────────────────────────────────────────────────────

/// Garde RAII sur un slot ring.
/// Au Drop, avance la séquence pour signaler la fin de la lecture/écriture.
pub struct SlotWriteGuard<'a> {
    cell: &'a SlotCell,
    /// Séquence à écrire au Drop (= pos + 1 pour signaler "données disponibles").
    commit_seq: u64,
    /// Abort au lieu de commit si Drop est appelé sans commit().
    aborted: bool,
}

impl<'a> SlotWriteGuard<'a> {
    /// Crée une garde d'écriture.
    /// `commit_seq` est pos + 1 pour un ring standard.
    pub fn new(cell: &'a SlotCell, commit_seq: u64) -> Self {
        Self {
            cell,
            commit_seq,
            aborted: false,
        }
    }

    /// Accède aux données du slot pour écriture.
    ///
    /// # Safety
    /// L'appelant est le seul producteur actif sur ce slot.
    pub unsafe fn data_mut(&self) -> &mut MaybeUninit<RingSlot> {
        self.cell.data_mut()
    }

    /// Confirme l'écriture.
    pub fn commit(mut self) {
        self.aborted = false;
        self.cell.store_seq(self.commit_seq);
        core::mem::forget(self); // évite le Drop
    }

    /// Annule l'écriture (restaure seq = commit_seq - 1).
    pub fn abort(mut self) {
        self.aborted = true;
        // Restaurer la séquence d'origine = pos (commit_seq - 1).
        self.cell.store_seq(self.commit_seq.wrapping_sub(1));
        core::mem::forget(self);
    }
}

impl<'a> Drop for SlotWriteGuard<'a> {
    fn drop(&mut self) {
        // Si ni commit ni abort n'ont été appelés, c'est un bug — paniquer en debug.
        if !self.aborted {
            // Restaurer pour éviter la corruption du ring.
            self.cell.store_seq(self.commit_seq.wrapping_sub(1));
            debug_assert!(
                false,
                "SlotWriteGuard droppé sans commit ni abort — ring corrompu"
            );
        }
    }
}

/// Garde RAII sur un slot ring en lecture.
pub struct SlotReadGuard<'a> {
    cell: &'a SlotCell,
    /// Séquence à écrire au Drop (= pos + RING_SIZE pour réutiliser le slot).
    release_seq: u64,
    released: bool,
}

impl<'a> SlotReadGuard<'a> {
    pub fn new(cell: &'a SlotCell, release_seq: u64) -> Self {
        Self {
            cell,
            release_seq,
            released: false,
        }
    }

    /// Accède aux données du slot en lecture.
    ///
    /// # Safety
    /// L'appelant est le seul consommateur actif sur ce slot.
    pub unsafe fn data_ref(&self) -> &MaybeUninit<RingSlot> {
        self.cell.data_ref()
    }

    /// Libère le slot (le réinjecte dans le ring pour le producteur suivant).
    pub fn release(mut self) {
        self.released = true;
        self.cell.store_seq(self.release_seq);
        core::mem::forget(self);
    }
}

impl<'a> Drop for SlotReadGuard<'a> {
    fn drop(&mut self) {
        if !self.released {
            self.cell.store_seq(self.release_seq);
            debug_assert!(
                false,
                "SlotReadGuard droppé sans release — slot jamais libéré"
            );
        }
    }
}
