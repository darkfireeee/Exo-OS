//! heap_free_map.rs — Carte de blocs libres pour le heap ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;

/// Carte de bits : 1 bit par bloc, 0=libre, 1=occupé.
pub struct HeapFreeMap {
    bits:         Vec<u64>,   // Chaque u64 couvre 64 blocs.
    total_blocks: u64,
    free_count:   u64,
}

impl HeapFreeMap {
    pub fn new(total_blocks: u64) -> Result<Self, FsError> {
        let n_words = (total_blocks.checked_add(63).ok_or(FsError::Overflow)? / 64) as usize;
        let mut bits = Vec::new();
        bits.try_reserve(n_words).map_err(|_| FsError::OutOfMemory)?;
        bits.resize(n_words, 0u64);
        Ok(Self { bits, total_blocks, free_count: total_blocks })
    }

    /// Cherche un run contigu de `n` blocs libres (first-fit).
    /// Retourne l'index du premier bloc du run, ou None.
    pub fn find_free_run(&self, n: u64) -> Option<u64> {
        if n == 0 { return Some(0); }
        let mut run_start = 0u64;
        let mut run_len   = 0u64;

        for block in 0..self.total_blocks {
            let word = (block / 64) as usize;
            let bit  = block % 64;
            let used = (self.bits[word] >> bit) & 1;
            if used == 0 {
                if run_len == 0 { run_start = block; }
                run_len += 1;
                if run_len >= n { return Some(run_start); }
            } else {
                run_len = 0;
            }
        }
        None
    }

    pub fn mark_used(&mut self, start: u64, n: u64) {
        for i in start..start.saturating_add(n).min(self.total_blocks) {
            let word = (i / 64) as usize;
            let bit  = i % 64;
            if self.bits[word] & (1 << bit) == 0 {
                self.bits[word] |= 1 << bit;
                self.free_count  = self.free_count.saturating_sub(1);
            }
        }
    }

    pub fn mark_free(&mut self, start: u64, n: u64) {
        for i in start..start.saturating_add(n).min(self.total_blocks) {
            let word = (i / 64) as usize;
            let bit  = i % 64;
            if self.bits[word] & (1 << bit) != 0 {
                self.bits[word] &= !(1 << bit);
                self.free_count += 1;
            }
        }
    }

    pub fn is_free(&self, block: u64) -> bool {
        if block >= self.total_blocks { return false; }
        let word = (block / 64) as usize;
        let bit  = block % 64;
        (self.bits[word] >> bit) & 1 == 0
    }

    pub fn free_blocks(&self) -> u64 { self.free_count }
    pub fn total_blocks(&self) -> u64 { self.total_blocks }
}
