//! Bitmap operations for block allocation tracking

use alloc::vec;
use alloc::vec::Vec;

/// Bitmap for tracking free/used blocks or inodes
pub struct Bitmap {
    /// Bitmap data (each u64 represents 64 bits)
    bits: Vec<u64>,

    /// Total number of bits
    len: usize,
}

impl Bitmap {
    /// Create new bitmap with given number of bits
    pub fn new(len: usize) -> Self {
        let words = (len + 63) / 64;
        Self {
            bits: vec![0; words],
            len,
        }
    }

    /// Create bitmap with all bits set
    pub fn new_all_set(len: usize) -> Self {
        let words = (len + 63) / 64;
        let mut bits = vec![!0u64; words];

        // Clear bits beyond len
        let remaining = len % 64;
        if remaining > 0 {
            bits[words - 1] = (1u64 << remaining) - 1;
        }

        Self { bits, len }
    }

    /// Get number of bits
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if bitmap is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Set bit at index
    #[inline]
    pub fn set(&mut self, index: usize) {
        if index >= self.len {
            return;
        }

        let word = index / 64;
        let bit = index % 64;
        self.bits[word] |= 1u64 << bit;
    }

    /// Clear bit at index
    #[inline]
    pub fn clear(&mut self, index: usize) {
        if index >= self.len {
            return;
        }

        let word = index / 64;
        let bit = index % 64;
        self.bits[word] &= !(1u64 << bit);
    }

    /// Test bit at index
    #[inline]
    pub fn test(&self, index: usize) -> bool {
        if index >= self.len {
            return false;
        }

        let word = index / 64;
        let bit = index % 64;
        (self.bits[word] & (1u64 << bit)) != 0
    }

    /// Find first zero bit
    pub fn find_first_zero(&self) -> Option<usize> {
        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word != !0u64 {
                let bit = word.trailing_ones() as usize;
                let index = word_idx * 64 + bit;

                if index < self.len {
                    return Some(index);
                }
            }
        }

        None
    }

    /// Find first set bit
    pub fn find_first_one(&self) -> Option<usize> {
        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                let bit = word.trailing_zeros() as usize;
                let index = word_idx * 64 + bit;

                if index < self.len {
                    return Some(index);
                }
            }
        }

        None
    }

    /// Find n contiguous zero bits
    pub fn find_contiguous_zeros(&self, n: usize) -> Option<usize> {
        if n == 0 || n > self.len {
            return None;
        }

        let mut start = 0;
        let mut count = 0;

        for i in 0..self.len {
            if !self.test(i) {
                if count == 0 {
                    start = i;
                }
                count += 1;

                if count == n {
                    return Some(start);
                }
            } else {
                count = 0;
            }
        }

        None
    }

    /// Count number of set bits
    pub fn count_ones(&self) -> usize {
        let mut count = 0;

        for &word in &self.bits {
            count += word.count_ones() as usize;
        }

        // Adjust for bits beyond len
        let total_bits = self.bits.len() * 64;
        if total_bits > self.len {
            let excess = total_bits - self.len;
            // Subtract excess bits if they were set
            let last_word = self.bits[self.bits.len() - 1];
            let excess_mask = !((1u64 << (64 - excess)) - 1);
            count -= (last_word & excess_mask).count_ones() as usize;
        }

        count
    }

    /// Count number of zero bits
    #[inline]
    pub fn count_zeros(&self) -> usize {
        self.len - self.count_ones()
    }

    /// Set range of bits
    pub fn set_range(&mut self, start: usize, len: usize) {
        for i in start..(start + len).min(self.len) {
            self.set(i);
        }
    }

    /// Clear range of bits
    pub fn clear_range(&mut self, start: usize, len: usize) {
        for i in start..(start + len).min(self.len) {
            self.clear(i);
        }
    }

    /// Clear all bits
    pub fn clear_all(&mut self) {
        for word in &mut self.bits {
            *word = 0;
        }
    }

    /// Set all bits
    pub fn set_all(&mut self) {
        for word in &mut self.bits {
            *word = !0u64;
        }

        // Clear bits beyond len
        let remaining = self.len % 64;
        if remaining > 0 {
            let last_idx = self.bits.len() - 1;
            self.bits[last_idx] = (1u64 << remaining) - 1;
        }
    }
}

impl core::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bitmap")
            .field("len", &self.len)
            .field("ones", &self.count_ones())
            .field("zeros", &self.count_zeros())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitmap_basic() {
        let mut bm = Bitmap::new(100);

        assert!(!bm.test(0));
        bm.set(0);
        assert!(bm.test(0));
        bm.clear(0);
        assert!(!bm.test(0));
    }

    #[test]
    fn test_find_first_zero() {
        let mut bm = Bitmap::new_all_set(100);
        bm.clear(42);

        assert_eq!(bm.find_first_zero(), Some(42));
    }

    #[test]
    fn test_contiguous_zeros() {
        let mut bm = Bitmap::new_all_set(100);
        bm.clear_range(10, 5);

        assert_eq!(bm.find_contiguous_zeros(5), Some(10));
        assert_eq!(bm.find_contiguous_zeros(6), None);
    }
}
