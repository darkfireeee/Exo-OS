// kernel/src/fs/exofs/storage/heap_free_map.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Carte de blocs libres du heap ExoFS — bitmap par tranches de 64 blocs
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Chaque bit de la bitmap représente un bloc de 4 KB dans le heap.
// Convention : 0 = libre, 1 = occupé (cohérent avec les allocateurs standard).
//
// Règles respectées :
// - OOM-02   : try_reserve(n) avant tout push/resize sur Vec.
// - ARITH-02 : checked_add pour les index de blocs.
// - LOCK-04  : la carte est protégée par SpinLock à l'extérieur (dans HeapAllocator).

use crate::fs::exofs::core::ExofsError;
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de blocs par mot bitmap (u64 = 64 bits).
const BLOCKS_PER_WORD: u64 = 64;

// ─────────────────────────────────────────────────────────────────────────────
// FreeRun — séquence de blocs libres contigus
// ─────────────────────────────────────────────────────────────────────────────

/// Description d'une séquence de blocs libres contigus.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FreeRun {
    /// Index du premier bloc (inclusif).
    pub start: u64,
    /// Nombre de blocs dans le run.
    pub len: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// HeapFreeMap
// ─────────────────────────────────────────────────────────────────────────────

/// Carte de bits décrivant l'état libre/occupé de chaque bloc du heap.
///
/// Convention : bit à 0 → bloc libre, bit à 1 → bloc occupé.
///
/// La taille est fixée à la création (`total_blocks`).
/// Les bits au-delà de `total_blocks` dans le dernier mot sont toujours à 1
/// pour ne jamais être sélectionnés comme libres.
pub struct HeapFreeMap {
    /// Tableau de mots bitmap (u64 → 64 blocs).
    bits: Vec<u64>,
    /// Nombre total de blocs représentés.
    total_blocks: u64,
    /// Nombre de blocs libres (mise à jour à chaque mark_used/mark_free).
    free_count: u64,
    /// Plus haut bloc utilisé connu (approximatif — optimisation du scan).
    high_water: u64,
}

impl HeapFreeMap {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Crée une carte où tous les blocs sont libres.
    ///
    /// # Règle OOM-02 : try_reserve avant resize.
    pub fn new(total_blocks: u64) -> Result<Self, ExofsError> {
        if total_blocks == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let n_words = words_for_blocks(total_blocks);
        let mut bits: Vec<u64> = Vec::new();
        bits.try_reserve(n_words)
            .map_err(|_| ExofsError::NoMemory)?;
        bits.resize(n_words, 0u64);

        // Masquer les bits au-delà de total_blocks dans le dernier mot.
        let rem = total_blocks % BLOCKS_PER_WORD;
        if rem != 0 && n_words > 0 {
            let last_valid_mask = (1u64 << rem) - 1;
            bits[n_words - 1] = !last_valid_mask; // bits au-delà = 1 (occupé)
        }

        Ok(Self {
            bits,
            total_blocks,
            free_count: total_blocks,
            high_water: 0,
        })
    }

    /// Crée une carte où tous les blocs sont occupés.
    ///
    /// Utilisé pour initialiser avant de marquer les blocs libres.
    pub fn new_full(total_blocks: u64) -> Result<Self, ExofsError> {
        if total_blocks == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let n_words = words_for_blocks(total_blocks);
        let mut bits: Vec<u64> = Vec::new();
        bits.try_reserve(n_words)
            .map_err(|_| ExofsError::NoMemory)?;
        bits.resize(n_words, u64::MAX); // tous occupés

        Ok(Self {
            bits,
            total_blocks,
            free_count: 0,
            high_water: total_blocks.saturating_sub(1),
        })
    }

    // ── Requêtes ─────────────────────────────────────────────────────────────

    /// Retourne `true` si le bloc `block` est libre.
    #[inline]
    pub fn is_free(&self, block: u64) -> bool {
        if block >= self.total_blocks {
            return false;
        }
        let (word, bit) = word_bit(block);
        (self.bits[word] >> bit) & 1 == 0
    }

    /// Retourne `true` si le bloc `block` est occupé.
    #[inline]
    pub fn is_used(&self, block: u64) -> bool {
        !self.is_free(block)
    }

    /// Nombre de blocs libres.
    #[inline]
    pub fn free_blocks(&self) -> u64 {
        self.free_count
    }

    /// Nombre de blocs occupés.
    #[inline]
    pub fn used_blocks(&self) -> u64 {
        self.total_blocks.saturating_sub(self.free_count)
    }

    /// Nombre total de blocs.
    #[inline]
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    /// Ratio d'utilisation en pourcentage (0..=100).
    pub fn used_pct(&self) -> u64 {
        if self.total_blocks == 0 {
            return 0;
        }
        (self.used_blocks() as u128 * 100 / self.total_blocks as u128) as u64
    }

    // ── Allocation (recherche) ────────────────────────────────────────────────

    /// Cherche un run de `n` blocs libres contigus — algorithme first-fit.
    ///
    /// Retourne l'index du premier bloc du run, ou `None`.
    ///
    /// # Règle ARITH-02 : checked_add pour run_len.
    pub fn find_free_run(&self, n: u64) -> Option<u64> {
        if n == 0 || n > self.free_count {
            return None;
        }

        let mut run_start: u64 = 0;
        let mut run_len: u64 = 0;

        let n_words = self.bits.len();
        let mut word_idx = 0;

        while word_idx < n_words {
            let word = self.bits[word_idx];

            // Optimisation : si le mot entier est plein, saute-le.
            if word == u64::MAX {
                run_len = 0;
                word_idx += 1;
                continue;
            }

            // Analyse bit par bit dans ce mot.
            for bit in 0u64..BLOCKS_PER_WORD {
                let block = (word_idx as u64)
                    .checked_mul(BLOCKS_PER_WORD)
                    .and_then(|w| w.checked_add(bit))?;

                if block >= self.total_blocks {
                    return None;
                }

                if (word >> bit) & 1 == 0 {
                    // Bloc libre.
                    if run_len == 0 {
                        run_start = block;
                    }
                    run_len = run_len.checked_add(1)?;
                    if run_len >= n {
                        return Some(run_start);
                    }
                } else {
                    // Bloc occupé — réinitialise le run.
                    run_len = 0;
                }
            }
            word_idx += 1;
        }
        None
    }

    /// Cherche un run de `n` blocs libres contigus en partant de `hint`.
    ///
    /// Essaie d'abord depuis `hint`, puis depuis le début si non trouvé.
    pub fn find_free_run_hint(&self, n: u64, hint: u64) -> Option<u64> {
        // Chercher depuis hint.
        if let Some(r) = self.find_free_run_from(n, hint) {
            return Some(r);
        }
        // Fallback depuis le début.
        if hint > 0 {
            self.find_free_run_from(n, 0)
        } else {
            None
        }
    }

    /// Cherche un run de `n` blocs libres en partant de `start_block`.
    fn find_free_run_from(&self, n: u64, start_block: u64) -> Option<u64> {
        let mut run_start = 0u64;
        let mut run_len = 0u64;

        for block in start_block..self.total_blocks {
            if self.is_free(block) {
                if run_len == 0 {
                    run_start = block;
                }
                run_len = run_len.checked_add(1)?;
                if run_len >= n {
                    return Some(run_start);
                }
            } else {
                run_len = 0;
            }
        }
        None
    }

    // ── Modification ──────────────────────────────────────────────────────────

    /// Marque `n` blocs à partir de `start` comme occupés.
    ///
    /// Retourne le nombre de blocs réellement marqués (peut être < n si débordement).
    pub fn mark_used(&mut self, start: u64, n: u64) -> u64 {
        let mut marked = 0u64;
        let end = start.saturating_add(n).min(self.total_blocks);

        for block in start..end {
            let (word, bit) = word_bit(block);
            if self.bits[word] & (1 << bit) == 0 {
                // Était libre → marquer occupé.
                self.bits[word] |= 1 << bit;
                self.free_count = self.free_count.saturating_sub(1);
            }
            marked = marked.saturating_add(1);

            // Mise à jour high_water.
            if block > self.high_water {
                self.high_water = block;
            }
        }
        marked
    }

    /// Marque `n` blocs à partir de `start` comme libres.
    ///
    /// Retourne le nombre de blocs réellement libérés.
    pub fn mark_free(&mut self, start: u64, n: u64) -> u64 {
        let mut freed = 0u64;
        let end = start.saturating_add(n).min(self.total_blocks);

        for block in start..end {
            let (word, bit) = word_bit(block);
            if self.bits[word] & (1 << bit) != 0 {
                // Était occupé → marquer libre.
                self.bits[word] &= !(1 << bit);
                self.free_count = self.free_count.saturating_add(1);
            }
            freed = freed.saturating_add(1);
        }
        freed
    }

    /// Marque un seul bloc comme occupé.
    #[inline]
    pub fn mark_one_used(&mut self, block: u64) {
        self.mark_used(block, 1);
    }

    /// Marque un seul bloc comme libre.
    #[inline]
    pub fn mark_one_free(&mut self, block: u64) {
        self.mark_free(block, 1);
    }

    // ── Analyse ───────────────────────────────────────────────────────────────

    /// Construit la liste des runs libres (OOM-02 : try_reserve).
    pub fn free_runs(&self) -> Result<Vec<FreeRun>, ExofsError> {
        let mut runs: Vec<FreeRun> = Vec::new();
        let mut run_start = 0u64;
        let mut in_run = false;

        for block in 0..self.total_blocks {
            if self.is_free(block) {
                if !in_run {
                    run_start = block;
                    in_run = true;
                }
            } else if in_run {
                let len = block - run_start;
                runs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                runs.push(FreeRun {
                    start: run_start,
                    len,
                });
                in_run = false;
            }
        }

        // Dernier run en cours.
        if in_run {
            runs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            runs.push(FreeRun {
                start: run_start,
                len: self.total_blocks - run_start,
            });
        }
        Ok(runs)
    }

    /// Nombre de runs libres (utile pour mesurer la fragmentation).
    pub fn free_run_count(&self) -> u64 {
        let mut count = 0u64;
        let mut in_run = false;

        for block in 0..self.total_blocks {
            if self.is_free(block) {
                if !in_run {
                    count += 1;
                    in_run = true;
                }
            } else {
                in_run = false;
            }
        }
        count
    }

    /// Longueur du plus grand run libre.
    pub fn largest_free_run(&self) -> u64 {
        let mut max_len = 0u64;
        let mut run_len = 0u64;

        for block in 0..self.total_blocks {
            if self.is_free(block) {
                run_len += 1;
                if run_len > max_len {
                    max_len = run_len;
                }
            } else {
                run_len = 0;
            }
        }
        max_len
    }

    /// Taux de fragmentation : 0 = compact, 100 = très fragmenté.
    pub fn fragmentation_pct(&self) -> u8 {
        if self.free_count == 0 {
            return 0;
        }
        let runs = self.free_run_count();
        if runs <= 1 {
            return 0;
        }
        // Fragmentation = (runs - 1) / free_count × 100.
        (((runs.saturating_sub(1)) as u128 * 100) / self.free_count as u128).min(100) as u8
    }

    /// Retourne le high-water mark (plus haut bloc alloué connu).
    #[inline]
    pub fn high_water(&self) -> u64 {
        self.high_water
    }

    // ── Persistance ───────────────────────────────────────────────────────────

    /// Retourne une référence sur le slice de mots bitmap.
    ///
    /// Utilisé pour la persistance de la carte de bits sur disque.
    #[inline]
    pub fn bitmap_words(&self) -> &[u64] {
        &self.bits
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires internes
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le nombre de mots u64 nécessaires pour `n` blocs.
#[inline]
fn words_for_blocks(n: u64) -> usize {
    ((n.saturating_add(BLOCKS_PER_WORD - 1)) / BLOCKS_PER_WORD) as usize
}

/// Retourne (index_mot, index_bit) pour un numéro de bloc.
#[inline]
fn word_bit(block: u64) -> (usize, u64) {
    ((block / BLOCKS_PER_WORD) as usize, block % BLOCKS_PER_WORD)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_all_free() {
        let m = HeapFreeMap::new(128).unwrap();
        assert_eq!(m.free_blocks(), 128);
        assert!(m.is_free(0));
        assert!(m.is_free(127));
    }

    #[test]
    fn test_mark_used_and_free() {
        let mut m = HeapFreeMap::new(64).unwrap();
        m.mark_used(10, 5);
        assert_eq!(m.free_blocks(), 59);
        assert!(!m.is_free(10));
        assert!(!m.is_free(14));
        assert!(m.is_free(15));

        m.mark_free(12, 2);
        assert_eq!(m.free_blocks(), 61);
        assert!(m.is_free(12));
    }

    #[test]
    fn test_find_free_run() {
        let mut m = HeapFreeMap::new(64).unwrap();
        m.mark_used(0, 10);
        let run = m.find_free_run(5);
        assert_eq!(run, Some(10));
    }

    #[test]
    fn test_find_free_run_none() {
        let mut m = HeapFreeMap::new(20).unwrap();
        m.mark_used(0, 20);
        assert!(m.find_free_run(1).is_none());
    }

    #[test]
    fn test_fragmentation_pct() {
        let mut m = HeapFreeMap::new(10).unwrap();
        // Pattern: libre, occupé, libre, occupé, libre → 3 runs libres.
        m.mark_used(1, 1);
        m.mark_used(3, 1);
        m.mark_used(5, 1);
        // Fragmentation = (3-1) / 7 × 100 ≈ 28%.
        assert!(m.fragmentation_pct() > 0);
    }

    #[test]
    fn test_full_boundary_bits() {
        // Vérifie que les blocs extra au-delà de total_blocks sont bien à "occupé".
        let m = HeapFreeMap::new(65).unwrap();
        // total_blocks = 65 → 2 mots (128 bits) → bits 65..127 = 1 (occupé).
        assert!(m.is_free(0));
        assert!(m.is_free(64));
        assert!(!m.is_free(65)); // hors bornes → false
    }

    #[test]
    fn test_largest_free_run() {
        let mut m = HeapFreeMap::new(20).unwrap();
        m.mark_used(5, 5); // blocs 5..9 occupés
                           // Deux runs libres : [0..4] (5 blocs) et [10..19] (10 blocs).
        assert_eq!(m.largest_free_run(), 10);
    }
}
