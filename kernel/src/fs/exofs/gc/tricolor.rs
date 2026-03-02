//! Marquage tricolore (blanc / gris / noir) pour le GC ExoFS.
//!
//! - Blanc  : non visité, candidat à la suppression après sweep.
//! - Gris   : racine atteinte, enfants pas encore visités.
//! - Noir   : entièrement visité, vivant garanti.
//!
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>
//! RÈGLE 14 : checked_add pour tout calcul d'offset/index.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::BlobId;

/// Couleur d'un nœud dans l'algorithme tricolore.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TricolorMark {
    /// Blanc : non visité (GC candidate).
    White = 0,
    /// Gris  : racine sûre, enfants pas encore visités.
    Grey  = 1,
    /// Noir  : entièrement visité, vivant.
    Black = 2,
}

impl TricolorMark {
    /// Retourne `true` si le blob est potentiellement reclaimable.
    #[inline]
    pub fn is_reclaimable(self) -> bool {
        self == TricolorMark::White
    }

    /// Retourne `true` si le blob doit encore être visité.
    #[inline]
    pub fn needs_visit(self) -> bool {
        self == TricolorMark::Grey
    }
}

/// Table de marquage compacte pour une passe GC.
///
/// Implémentée comme vecteur de slots (BlobId → TricolorMark).
/// Une epoch-GC alloue cet ensemble, le remplit, puis le libère.
pub struct TricolorSet {
    /// Bits compressés : 2 bits par entrée → 32 entrées par u64.
    /// index_in_bitset = blob_idx / 32, bit_offset = (blob_idx % 32) * 2.
    bits: Vec<AtomicU64>,
    /// Nombre total de BlobIds enregistrés.
    capacity: usize,
    /// Compteur de blobs gris actifs (pour savoir si le grey-set est vide).
    grey_count: AtomicU64,
}

const BLOBS_PER_WORD: usize = 32; // 64 bits / 2 bits/blob

impl TricolorSet {
    /// Cré un nouvel ensemble pour `n` blobs, tous blancs initialement.
    pub fn new(n: usize) -> Result<Self, crate::fs::exofs::core::FsError> {
        let words = n
            .checked_add(BLOBS_PER_WORD - 1)
            .ok_or(crate::fs::exofs::core::FsError::Overflow)?
            / BLOBS_PER_WORD;
        let mut bits = Vec::new();
        bits.try_reserve(words)
            .map_err(|_| crate::fs::exofs::core::FsError::OutOfMemory)?;
        for _ in 0..words {
            bits.push(AtomicU64::new(0)); // 0 = tous blancs
        }
        Ok(Self {
            bits,
            capacity: n,
            grey_count: AtomicU64::new(0),
        })
    }

    /// Retourne la couleur actuelle d'un blob par son index.
    pub fn get(&self, idx: usize) -> TricolorMark {
        debug_assert!(idx < self.capacity);
        let word = idx / BLOBS_PER_WORD;
        let shift = (idx % BLOBS_PER_WORD) * 2;
        let bits = self.bits[word].load(Ordering::Acquire);
        match (bits >> shift) & 0b11 {
            0 => TricolorMark::White,
            1 => TricolorMark::Grey,
            _ => TricolorMark::Black,
        }
    }

    /// Marque un blob comme gris (racine atteinte).
    /// Retourne `true` si la couleur a changé (était blanc).
    pub fn mark_grey(&self, idx: usize) -> bool {
        self.set_color(idx, TricolorMark::Grey, TricolorMark::White)
    }

    /// Marque un blob comme noir (totalement visité).
    /// Retourne `true` si la couleur a changé (était gris).
    pub fn mark_black(&self, idx: usize) -> bool {
        let changed = self.set_color(idx, TricolorMark::Black, TricolorMark::Grey);
        if changed {
            // Décrémente grey_count — ne peut pas underflow si l'algorithme est correct.
            let prev = self.grey_count.fetch_sub(1, Ordering::Relaxed);
            if prev == 0 {
                panic!("[ExoFS GC] grey_count underflow — invariant tricolore cassé");
            }
        }
        changed
    }

    /// Retourne `true` si l'ensemble gris est vide (passe terminée).
    #[inline]
    pub fn grey_set_empty(&self) -> bool {
        self.grey_count.load(Ordering::Acquire) == 0
    }

    /// Applique `f` sur l'index de chaque blob blanc → liste des reclaimables.
    pub fn collect_white<F>(&self, mut f: F)
    where
        F: FnMut(usize),
    {
        for (wi, word) in self.bits.iter().enumerate() {
            let mut w = word.load(Ordering::Acquire);
            let base = wi * BLOBS_PER_WORD;
            let mut slot = 0usize;
            while w != 0 || slot < BLOBS_PER_WORD {
                let color = (w & 0b11) as u8;
                let idx = base
                    .checked_add(slot)
                    .expect("[ExoFS GC] overflow index collect_white");
                if idx >= self.capacity {
                    break;
                }
                if color == 0 {
                    f(idx);
                }
                w >>= 2;
                slot += 1;
            }
        }
    }

    // ------------------------------------------------------------------
    // Interne
    // ------------------------------------------------------------------

    fn set_color(&self, idx: usize, new: TricolorMark, expected: TricolorMark) -> bool {
        debug_assert!(idx < self.capacity);
        let word = idx / BLOBS_PER_WORD;
        let shift = (idx % BLOBS_PER_WORD) * 2;
        let mask: u64 = 0b11 << shift;
        let new_bits = (new as u64) << shift;
        let exp_bits = (expected as u64) << shift;

        let mut current = self.bits[word].load(Ordering::Acquire);
        loop {
            if (current & mask) != exp_bits {
                return false; // déjà changé
            }
            let desired = (current & !mask) | new_bits;
            match self.bits[word].compare_exchange_weak(
                current, desired, Ordering::AcqRel, Ordering::Acquire,
            ) {
                Ok(_) => {
                    if new == TricolorMark::Grey {
                        self.grey_count.fetch_add(1, Ordering::Relaxed);
                    }
                    return true;
                }
                Err(v) => current = v,
            }
        }
    }
}

/// Correspondance BlobId → index dans TricolorSet.
///
/// Utilisé par EpochScanner pour indexer les blobs de la passe en cours.
pub struct BlobIndex {
    entries: Vec<BlobId>,
}

impl BlobIndex {
    /// Crée un index vide.
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Enregistre un BlobId et retourne son index.
    pub fn register(
        &mut self,
        id: BlobId,
    ) -> Result<usize, crate::fs::exofs::core::FsError> {
        let idx = self.entries.len();
        self.entries
            .try_reserve(1)
            .map_err(|_| crate::fs::exofs::core::FsError::OutOfMemory)?;
        self.entries.push(id);
        Ok(idx)
    }

    /// Retrouve l'index d'un BlobId (recherche linéaire — acceptable pour GC hors chemin critique).
    pub fn index_of(&self, id: &BlobId) -> Option<usize> {
        self.entries.iter().position(|b| b == id)
    }

    /// Retourne le BlobId correspondant à l'index.
    pub fn blob_at(&self, idx: usize) -> Option<&BlobId> {
        self.entries.get(idx)
    }

    /// Nombre de blobs enregistrés.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
