// kernel/src/security/crypto/rng.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CSPRNG — Cryptographically secure pseudo-random number generator
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • Primary source: RDRAND (Intel) — hardware CPU instruction
//   • Generator: ChaCha20 block function (RFC 8439) imported from
//     xchacha20_poly1305.rs — pure u32 arithmetic, zero SIMD/SSE2.
//   • Reseed: every 4096 ChaCha20 blocks generated, RDRAND mixing.
//   • Fallback: TSC + stack address when RDRAND fails.
//
// RULE CRYPTO-CRATES: ChaCha20 is the ONLY allowed in-house crypto primitive
// (the chacha20 crate triggers LLVM ERROR: split 128-bit on
// x86_64-unknown-none without SSE2). The implementation conforms to RFC 8439.
//
// RULE RNG-01: NEVER call rng_fill() from an NMI context (no lock).
// RULE RNG-02: Always check RDRAND return (CF flag).
// RULE RNG-03: In case of RDRAND failure after 10 attempts → fallback to TSC+stack.
// ═══════════════════════════════════════════════════════════════════════════════

use super::xchacha20_poly1305::chacha20_block;
use crate::arch::x86_64::cpu::features::cpu_features_or_none;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Number of ChaCha20 blocks before mandatory reseed.
const RESEED_INTERVAL_BLOCKS: u64 = 4096;

/// ChaCha20 block size in bytes.
const CHACHA20_BLOCK_SIZE: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// RdrandError
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum RngError {
    /// RDRAND failed after the maximum number of attempts.
    RdrandExhausted,
    /// RNG not initialized.
    NotInitialized,
    /// Insufficient entropy at startup.
    InsufficientEntropy,
}

// ─────────────────────────────────────────────────────────────────────────────
// RDRAND — hardware read
// ─────────────────────────────────────────────────────────────────────────────

/// Attempts to read a 64-bit value from RDRAND.
/// Returns Ok(value) or Err if CF=0 (retries exhausted).
#[inline]
fn rdrand64() -> Result<u64, RngError> {
    #[cfg(target_arch = "x86_64")]
    {
        // Avoids #UD if the RDRAND instruction is not supported by CPU/VM.
        if !cpu_features_or_none().map_or(false, |features| features.has_rdrand()) {
            return Err(RngError::RdrandExhausted);
        }

        for _ in 0..10 {
            // SAFETY: CPUID confirmed RDRAND support above.
            if let Some(val) = unsafe { crate::arch::x86_64::rdrand64_once() } {
                return Ok(val);
            }
            crate::arch::x86_64::cpu_pause();
        }
        Err(RngError::RdrandExhausted)
    }
    #[cfg(not(target_arch = "x86_64"))]
    Err(RngError::RdrandExhausted)
}

/// Attempts to read a 64-bit value from RDSEED.
///
/// RDSEED exposes the raw HARDWARE entropy source (vs RDRAND = reseeded
/// hardware CSPRNG), hence superior entropy quality for seeding.
/// Returns `Err` if unsupported or CF=0 after 10 attempts.
#[inline]
fn rdseed64() -> Result<u64, RngError> {
    #[cfg(target_arch = "x86_64")]
    {
        if !cpu_features_or_none().map_or(false, |features| features.has_rdseed()) {
            return Err(RngError::RdrandExhausted);
        }
        for _ in 0..10 {
            // SAFETY: CPUID confirmed RDSEED support above.
            if let Some(val) = unsafe { crate::arch::x86_64::rdseed64_once() } {
                return Ok(val);
            }
            crate::arch::x86_64::cpu_pause();
        }
        Err(RngError::RdrandExhausted)
    }
    #[cfg(not(target_arch = "x86_64"))]
    Err(RngError::RdrandExhausted)
}

/// Reads n bytes from RDRAND into a buffer.
pub fn rdrand_fill(buf: &mut [u8]) -> Result<(), RngError> {
    let mut pos = 0;
    while pos + 8 <= buf.len() {
        let val = rdrand64()?;
        buf[pos..pos + 8].copy_from_slice(&val.to_le_bytes());
        pos += 8;
    }
    if pos < buf.len() {
        let val = rdrand64()?;
        let bytes = val.to_le_bytes();
        let remaining = buf.len() - pos;
        buf[pos..].copy_from_slice(&bytes[..remaining]);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — TSC and stack pointer (entropy fallback)
// ─────────────────────────────────────────────────────────────────────────────

/// Reads the TSC (Time Stamp Counter).
#[inline]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::tsc::read_tsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Reads current stack pointer (additional entropy source).
#[inline]
fn read_sp() -> u64 {
    crate::arch::x86_64::read_rsp()
}

/// Gathers entropy from **all** available sources into a pool,
/// then **conditions it via Blake3** → 32-byte seed. Returns `true` if at
/// least one HARDWARE source (RDSEED/RDRAND) contributed.
///
/// Robustness (hardened RULE RNG-03): the previous seed fallback derived ONLY
/// from TSC+SP via ad-hoc mixing (predictable, unwhitened). Here:
///   1. RDSEED (true hardware entropy) ×4, then RDRAND ×6;
///   2. **TSC jitter** — repeated reads interleaved with PAUSE; lower
///      bits vary (clock jitter) = real source of temporal randomness;
///   3. stack pointer (addressing entropy / KASLR);
///   4. **Blake3 conditioning** of the entire pool → 32 bytes whitened.
/// Even without RDRAND, the seed is cryptographically conditioned and aggregates
/// multiple sources (raised entropy floor vs raw TSC).
fn gather_seed(out: &mut [u8; 32]) -> bool {
    fn push(pool: &mut [u8; 160], off: &mut usize, v: u64) {
        if *off + 8 <= pool.len() {
            pool[*off..*off + 8].copy_from_slice(&v.to_le_bytes());
            *off += 8;
        }
    }

    let mut pool = [0u8; 160];
    let mut off = 0usize;
    let mut hw = false;

    // 1. RDSEED — "true" hardware entropy.
    for _ in 0..4 {
        if let Ok(v) = rdseed64() {
            push(&mut pool, &mut off, v);
            hw = true;
        }
    }
    // 2. RDRAND — hardware CSPRNG.
    for _ in 0..6 {
        if let Ok(v) = rdrand64() {
            push(&mut pool, &mut off, v);
            hw = true;
        }
    }
    // 3. TSC Jitter (temporal randomness) + 4. stack pointer.
    for _ in 0..8 {
        push(&mut pool, &mut off, read_tsc());
        crate::arch::x86_64::cpu_pause();
    }
    push(&mut pool, &mut off, read_sp());

    // 5. Blake3 conditioning → 32 whitened bytes.
    let conditioned = super::blake3::blake3_hash(&pool[..off]);
    out.copy_from_slice(&conditioned);

    // Clear pool (may contain sensitive residual entropy).
    for b in pool.iter_mut() {
        // SAFETY: write_volatile prevents erasure elision.
        unsafe {
            core::ptr::write_volatile(b, 0);
        }
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    hw
}

// ─────────────────────────────────────────────────────────────────────────────
// ChaCha20 CSPRNG — Generator based on ChaCha20 block function
// ─────────────────────────────────────────────────────────────────────────────

/// CSPRNG based on ChaCha20 block function (RFC 8439).
///
/// Generates a ChaCha20 keystream from a 256-bit key and a 96-bit nonce
/// + counter. Each call to `chacha20_block` produces 64 bytes
/// of cryptographically secure random data.
struct ChaCha20Csprng {
    /// ChaCha20 key (256 bits).
    key: [u8; 32],
    /// ChaCha20 nonce (96 bits).
    nonce: [u8; 12],
    /// Current ChaCha20 block counter.
    counter: u32,
    /// Buffer of last generated ChaCha20 block (64 bytes).
    buffer: [u8; CHACHA20_BLOCK_SIZE],
    /// Consumption position in buffer.
    buffer_pos: usize,
    /// Total number of blocks generated since last reseed.
    blocks_since_reseed: u64,
}

impl ChaCha20Csprng {
    const fn new() -> Self {
        Self {
            key: [0u8; 32],
            nonce: [0u8; 12],
            counter: 0,
            buffer: [0u8; CHACHA20_BLOCK_SIZE],
            buffer_pos: CHACHA20_BLOCK_SIZE, // Forces generation on first call
            blocks_since_reseed: 0,
        }
    }

    /// Initializes CSPRNG with a 32-byte seed.
    fn seed(&mut self, entropy: &[u8; 32]) {
        self.key.copy_from_slice(entropy);
        // Derives a nonce from the seed using last 12 bytes XOR
        for i in 0..12 {
            self.nonce[i] = entropy[i]
                .wrapping_add(entropy[i + 20])
                .wrapping_add(entropy[(i + 7) % 32]);
        }
        self.counter = 0;
        self.buffer_pos = CHACHA20_BLOCK_SIZE; // Forces regeneration
        self.blocks_since_reseed = 0;
    }

    /// Reseeds the CSPRNG with new entropy data.
    ///
    /// XORs new entropy into key and advances nonce to
    /// guarantee a different keystream after reseed.
    fn reseed(&mut self, extra: &[u8; 32]) {
        for i in 0..32 {
            self.key[i] ^= extra[i];
        }
        // Advances nonce to guarantee a new keystream
        self.nonce[11] = self.nonce[11].wrapping_add(1);
        if self.nonce[11] == 0 {
            self.nonce[10] = self.nonce[10].wrapping_add(1);
        }
        self.counter = 0;
        self.buffer_pos = CHACHA20_BLOCK_SIZE; // Forces regeneration
        self.blocks_since_reseed = 0;
    }

    /// Generates a new ChaCha20 block and refills buffer.
    #[inline]
    fn refill_buffer(&mut self) {
        self.buffer = chacha20_block(&self.key, &self.nonce, self.counter);
        self.counter = self.counter.wrapping_add(1);
        self.buffer_pos = 0;
        self.blocks_since_reseed += 1;
    }

    /// Fills a buffer with random bytes from the ChaCha20 keystream.
    fn fill(&mut self, buf: &mut [u8]) {
        let mut pos = 0;
        while pos < buf.len() {
            if self.buffer_pos >= CHACHA20_BLOCK_SIZE {
                self.refill_buffer();
            }
            let remaining = buf.len() - pos;
            let available = CHACHA20_BLOCK_SIZE - self.buffer_pos;
            let to_copy = remaining.min(available);
            buf[pos..pos + to_copy]
                .copy_from_slice(&self.buffer[self.buffer_pos..self.buffer_pos + to_copy]);
            self.buffer_pos += to_copy;
            pos += to_copy;
        }
    }

    /// Returns the number of blocks generated since last reseed.
    #[inline]
    fn blocks_since_reseed(&self) -> u64 {
        self.blocks_since_reseed
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelRng — global singleton
// ─────────────────────────────────────────────────────────────────────────────

struct KernelRng {
    prng: ChaCha20Csprng,
    initialized: bool,
    /// True if seed (initial or reseed) received HARDWARE entropy
    /// (RDSEED/RDRAND). False = conditioned seed but degraded entropy.
    hw_seeded: bool,
    bytes_generated: u64,
    reseed_count: u64,
}

impl KernelRng {
    const fn new() -> Self {
        Self {
            prng: ChaCha20Csprng::new(),
            initialized: false,
            hw_seeded: false,
            bytes_generated: 0,
            reseed_count: 0,
        }
    }

    fn init(&mut self) {
        let mut seed = [0u8; 32];
        // Seed = multi-source pool (RDSEED+RDRAND+TSC jitter+SP) Blake3 conditioned.
        self.hw_seeded = gather_seed(&mut seed);
        self.prng.seed(&seed);
        // Zeroize seed on stack (write_volatile to prevent elision).
        for b in seed.iter_mut() {
            // SAFETY: bounded erasure of local buffer.
            unsafe {
                core::ptr::write_volatile(b, 0);
            }
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        self.initialized = true;
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), RngError> {
        if !self.initialized {
            return Err(RngError::NotInitialized);
        }

        // Reseed every RESEED_INTERVAL_BLOCKS blocks — same multi-source pool
        // Blake3 conditioned (never weak unwhitened fallback).
        if self.prng.blocks_since_reseed() >= RESEED_INTERVAL_BLOCKS {
            let mut extra = [0u8; 32];
            let hw = gather_seed(&mut extra);
            self.prng.reseed(&extra);
            self.hw_seeded = self.hw_seeded || hw;
            for b in extra.iter_mut() {
                // SAFETY: bounded erasure of local buffer.
                unsafe {
                    core::ptr::write_volatile(b, 0);
                }
            }
            core::sync::atomic::fence(Ordering::SeqCst);
            self.reseed_count += 1;
        }

        self.prng.fill(buf);
        self.bytes_generated += buf.len() as u64;
        Ok(())
    }
}

static KERNEL_RNG: Mutex<KernelRng> = Mutex::new(KernelRng::new());
static RNG_INIT: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the kernel random number generator.
/// Must be called after CPU initialization (RDRAND available).
pub fn rng_init() {
    if RNG_INIT.swap(true, Ordering::SeqCst) {
        return; // Already initialized
    }
    KERNEL_RNG.lock().init();
}

/// Fills a buffer with cryptographically random bytes.
pub fn rng_fill(buf: &mut [u8]) -> Result<(), RngError> {
    KERNEL_RNG.lock().fill(buf)
}

/// Generates a random u64.
pub fn rng_u64() -> Result<u64, RngError> {
    let mut buf = [0u8; 8];
    rng_fill(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

/// Generates a random u32.
pub fn rng_u32() -> Result<u32, RngError> {
    let mut buf = [0u8; 4];
    rng_fill(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

/// Generates an array of 32 random bytes (cryptographic key).
pub fn rng_key32() -> Result<[u8; 32], RngError> {
    let mut key = [0u8; 32];
    rng_fill(&mut key)?;
    Ok(key)
}

/// Generates a 24-byte nonce (XChaCha20).
pub fn rng_nonce24() -> Result<[u8; 24], RngError> {
    let mut nonce = [0u8; 24];
    rng_fill(&mut nonce)?;
    Ok(nonce)
}

/// Returns true if RNG is initialized and operational.
#[inline(always)]
pub fn rng_is_ready() -> bool {
    RNG_INIT.load(Ordering::Acquire) && KERNEL_RNG.lock().initialized
}

/// Returns true if RNG was seeded with HARDWARE entropy
/// (RDSEED/RDRAND). False = Blake3 conditioned seed but without hardware source
/// (degraded entropy — a caller generating a critical long-term key may
/// choose to defer/alert on this basis).
#[inline(always)]
pub fn rng_hw_seeded() -> bool {
    KERNEL_RNG.lock().hw_seeded
}

/// RNG statistics.
#[derive(Debug, Clone, Copy)]
pub struct RngStats {
    pub bytes_generated: u64,
    pub reseed_count: u64,
    /// Hardware entropy obtained at seeding (RDSEED/RDRAND).
    pub hw_seeded: bool,
}

pub fn rng_stats() -> RngStats {
    let rng = KERNEL_RNG.lock();
    RngStats {
        bytes_generated: rng.bytes_generated,
        reseed_count: rng.reseed_count,
        hw_seeded: rng.hw_seeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed is Blake3 conditioned (never all-zero) and varies between two
    /// calls (TSC jitter + hardware sources) — no constant weak seed.
    #[test]
    fn gather_seed_is_conditioned_and_varies() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        let _ = gather_seed(&mut a);
        let _ = gather_seed(&mut b);
        assert_ne!(a, [0u8; 32], "seed conditionné ne doit pas être tout-zéro");
        assert_ne!(a, b, "deux seeds successifs doivent différer (TSC/HW)");
    }

    /// After init, CSPRNG produces a non-zero stream that varies from one draw to another.
    #[test]
    fn rng_fill_is_nonzero_and_varies() {
        rng_init();
        let mut x = [0u8; 32];
        let mut y = [0u8; 32];
        rng_fill(&mut x).expect("rng ready");
        rng_fill(&mut y).expect("rng ready");
        assert_ne!(x, [0u8; 32]);
        assert_ne!(x, y);
    }
}
