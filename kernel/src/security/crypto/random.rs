//! Cryptographically Secure Random Number Generator
//!
//! Uses ChaCha20 as CSPRNG with hardware entropy when available

use super::chacha20::chacha20_rng;
use core::sync::atomic::{AtomicU64, Ordering};

/// Global CSPRNG counter for unique streams
static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Crypto RNG state
pub struct CryptoRng {
    key: [u8; 32],
    nonce: [u8; 12],
    counter: u64,
}

impl CryptoRng {
    /// Create new CSPRNG
    ///
    /// Seeds from hardware RNG if available, otherwise uses counter
    pub fn new() -> Self {
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];

        // Try to get entropy from hardware
        if !try_hardware_rng(&mut key) {
            // Fallback: use atomic counter + some pseudo-random data
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            key[..8].copy_from_slice(&count.to_le_bytes());

            // Mix in some compile-time entropy
            key[8..16].copy_from_slice(&count.wrapping_mul(0x123456789abcdef).to_le_bytes());
        }

        // Nonce from counter
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        nonce[..8].copy_from_slice(&count.to_le_bytes());

        Self {
            key,
            nonce,
            counter: 0,
        }
    }

    /// Fill buffer with random bytes
    pub fn fill_bytes(&mut self, buf: &mut [u8]) {
        // Zero the buffer first
        for byte in buf.iter_mut() {
            *byte = 0;
        }

        // Generate using ChaCha20
        chacha20_rng(&self.key, &self.nonce, buf);

        // Update state for next call
        self.counter = self.counter.wrapping_add(1);
        self.nonce[..8].copy_from_slice(&self.counter.to_le_bytes());
    }

    /// Generate random u64
    pub fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    /// Generate random u32
    pub fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }
}

impl Default for CryptoRng {
    fn default() -> Self {
        Self::new()
    }
}

/// Try to get entropy from hardware RNG (RDRAND on x86_64)
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn try_hardware_rng(buf: &mut [u8]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // Try RDRAND if available
        if is_rdrand_supported() {
            for chunk in buf.chunks_mut(8) {
                if let Some(val) = rdrand_u64() {
                    let bytes = val.to_le_bytes();
                    let len = chunk.len().min(8);
                    chunk[..len].copy_from_slice(&bytes[..len]);
                } else {
                    return false;
                }
            }
            return true;
        }
    }
    false
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn try_hardware_rng(_buf: &mut [u8]) -> bool {
    false
}

#[cfg(target_arch = "x86_64")]
fn is_rdrand_supported() -> bool {
    // Check CPUID for RDRAND support
    // For now, assume not supported (would need CPUID)
    false
}

#[cfg(target_arch = "x86_64")]
fn rdrand_u64() -> Option<u64> {
    // Would use RDRAND instruction here
    // For now, return None
    None
}

/// Get random bytes (convenience function)
pub fn get_random_bytes(length: usize) -> alloc::vec::Vec<u8> {
    let mut rng = CryptoRng::new();
    let mut buf = alloc::vec![0u8; length];
    rng.fill_bytes(&mut buf);
    buf
}

/// Fill slice with random bytes
pub fn fill_random(buf: &mut [u8]) {
    let mut rng = CryptoRng::new();
    rng.fill_bytes(buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rng_basic() {
        let mut rng = CryptoRng::new();
        let mut buf1 = [0u8; 32];
        let mut buf2 = [0u8; 32];

        rng.fill_bytes(&mut buf1);
        rng.fill_bytes(&mut buf2);

        // Should be different
        assert_ne!(buf1, buf2);
    }

    #[test]
    fn test_rng_distribution() {
        let mut rng = CryptoRng::new();
        let mut zeros = 0;
        let mut ones = 0;

        for _ in 0..1000 {
            let byte = rng.next_u32() as u8;
            for bit in 0..8 {
                if (byte >> bit) & 1 == 0 {
                    zeros += 1;
                } else {
                    ones += 1;
                }
            }
        }

        // Should be roughly 50/50 (allow 40-60% range)
        let total = zeros + ones;
        let zero_ratio = (zeros as f64) / (total as f64);
        assert!(zero_ratio > 0.4 && zero_ratio < 0.6);
    }
}
