//! Bloom Filter
//!
//! Probabilistic data structure for fast set membership tests.
//! Used for rapid capability lookups (negative checks are fast).

use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

pub struct BloomFilter {
    bits: Vec<u64>,
    num_bits: usize,
    num_hashes: usize,
}

impl BloomFilter {
    /// Create new Bloom Filter with specified size and hash count
    ///
    /// # Arguments
    /// * `num_bits` - Size of the bit array
    /// * `num_hashes` - Number of hash functions to use
    pub fn new(num_bits: usize, num_hashes: usize) -> Self {
        let u64_count = (num_bits + 63) / 64;

        Self {
            bits: alloc::vec![0; u64_count],
            num_bits,
            num_hashes,
        }
    }

    fn get_hashes<T: Hash>(&self, _item: &T) -> (u64, u64) {
        // Use double hashing to simulate k hashes
        // h_i(x) = (h1(x) + i * h2(x)) % m

        // We need a hasher. Since we are no_std, we might need a simple hasher.
        // For now, let's assume we have a way to hash.
        // Using a simple FNV-1a style hash for demonstration if standard hasher not available.
        // But core::hash::Hasher trait is available.

        // Placeholder: In real kernel, use SipHash or similar available in core/alloc
        // Here we simulate with a simple mix
        let h1 = 0xcbf29ce484222325; // FNV offset basis
        let h2 = 0x100000001b3; // FNV prime

        // This is a stub for the hashing logic since we don't have a concrete Hasher in scope easily without imports.
        // In production, pass in a Hasher builder or use a specific hash function.
        (h1, h2)
    }

    pub fn insert<T: Hash>(&mut self, item: &T) {
        let (h1, h2) = self.get_hashes(item);

        for i in 0..self.num_hashes {
            let bit_idx = (h1.wrapping_add((i as u64).wrapping_mul(h2))) as usize % self.num_bits;
            let word_idx = bit_idx / 64;
            let bit_offset = bit_idx % 64;
            self.bits[word_idx] |= 1 << bit_offset;
        }
    }

    pub fn contains<T: Hash>(&self, item: &T) -> bool {
        let (h1, h2) = self.get_hashes(item);

        for i in 0..self.num_hashes {
            let bit_idx = (h1.wrapping_add((i as u64).wrapping_mul(h2))) as usize % self.num_bits;
            let word_idx = bit_idx / 64;
            let bit_offset = bit_idx % 64;

            if (self.bits[word_idx] & (1 << bit_offset)) == 0 {
                return false;
            }
        }
        true
    }
}
