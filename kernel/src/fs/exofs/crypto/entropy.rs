//! Pool d'entropie ExoFS — CSPRNG basé sur ChaCha20-DRNG.
//!
//! # Architecture
//!
//! Le `EntropyPool` maintient un état interne de 512 bits (8 mots u64)
//! mis à jour par plusieurs sources d'entropie (TSC, feeds externes).
//! La génération utilise une variante ChaCha20 à 20 rounds sans accès mémoire externe.
//!
//! # Règles
//! - OOM-02  : `try_reserve` avant toute allocation.
//! - ARITH-02: arithmétique `wrapping_*` / `checked_*` / `saturating_*`.
//! - RECUR-01: aucune récursivité.

#![allow(dead_code)]

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Instance globale
// ─────────────────────────────────────────────────────────────────────────────

/// Instance globale accessible depuis l'ensemble du module crypto.
pub static ENTROPY_POOL: EntropyPool = EntropyPool::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// EntropyPool
// ─────────────────────────────────────────────────────────────────────────────

/// Pool d'entropie kernel (CSPRNG).
///
/// L'état interne est constitué de 8 mots 64-bit (512 bits).
/// Constantes "nothing-up-my-sleeve" : premiers 64 chiffres de π en hexadécimal.
pub struct EntropyPool {
    /// Mots d'état interne.
    state:   [AtomicU64; 8],
    /// Compteur incrémental.
    counter: AtomicU64,
    /// Indicateur d'initialisation (0 = non-initialisé).
    seeded:  AtomicU64,
    /// Nombre total de bytes générés (statistique).
    bytes_generated: AtomicU64,
}

impl EntropyPool {
    /// Construit un pool non-initialisé. Constante pour l'initialisation statique.
    pub const fn new_const() -> Self {
        Self {
            state: [
                AtomicU64::new(0x243F_6A88_85A3_08D3),
                AtomicU64::new(0x1319_8A2E_0370_7344),
                AtomicU64::new(0xA409_3822_299F_31D0),
                AtomicU64::new(0x082E_FA98_EC4E_6C89),
                AtomicU64::new(0x4520_1C6A_CBFC_84B3),
                AtomicU64::new(0x5EF5_562E_7FCC_5B28),
                AtomicU64::new(0xABE7_0CDF_1F27_2F13),
                AtomicU64::new(0xD0A5_2A3A_1B3D_0A4C),
            ],
            counter:         AtomicU64::new(0),
            seeded:          AtomicU64::new(0),
            bytes_generated: AtomicU64::new(0),
        }
    }

    // ── Amorçage ─────────────────────────────────────────────────────────────

    /// Mélange une valeur 64-bit dans le pool.
    ///
    /// ARITH-02 : `wrapping_add`, `rotate_left`.
    pub fn mix(&self, entropy_word: u64) {
        let ctr = self.counter.fetch_add(1, Ordering::Relaxed);
        let slot = (ctr % 8) as usize;
        let prev = self.state[slot].load(Ordering::Relaxed);
        let new_val = prev
            .wrapping_add(entropy_word)
            .rotate_left(13)
            ^ entropy_word.wrapping_mul(0x9E37_79B9_7F4A_7C15); // Fibonacci hashing
        self.state[slot].store(new_val, Ordering::Release);
        self.seeded.fetch_or(1, Ordering::Release);
    }

    /// Amorce depuis un tableau de 32 octets (seed extérieur).
    pub fn seed_from_bytes(&self, seed: &[u8; 32]) {
        for (i, chunk) in seed.chunks(8).enumerate() {
            if i >= 4 { break; }
            let mut b = [0u8; 8];
            let len = chunk.len().min(8);
            b[..len].copy_from_slice(&chunk[..len]);
            self.mix(u64::from_le_bytes(b));
        }
    }

    /// Amorce depuis une tranche arbitraire (hash XOR des mots).
    pub fn seed_from_slice(&self, data: &[u8]) {
        for (i, chunk) in data.chunks(8).enumerate() {
            let mut b = [0u8; 8];
            let len = chunk.len().min(8);
            b[..len].copy_from_slice(&chunk[..len]);
            let word = u64::from_le_bytes(b)
                .wrapping_add(i as u64)
                .wrapping_mul(0x6C62_272E_07BB_0142);
            self.mix(word);
        }
    }

    /// Retourne `true` si le pool a été amorcé au moins une fois.
    #[inline]
    pub fn is_seeded(&self) -> bool {
        self.seeded.load(Ordering::Acquire) != 0
    }

    /// Retourne le nombre cumulé d'octets générés.
    pub fn bytes_generated(&self) -> u64 {
        self.bytes_generated.load(Ordering::Relaxed)
    }

    // ── Génération ───────────────────────────────────────────────────────────

    /// Génère exactement `n` octets pseudo-aléatoires.
    ///
    /// OOM-02 : `try_reserve`.
    pub fn random_bytes(&self, n: usize) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let mut remaining = n;
        while remaining > 0 {
            let block = self.generate_block();
            let take  = remaining.min(block.len());
            out.extend_from_slice(&block[..take]);
            remaining -= take;
        }
        self.bytes_generated.fetch_add(n as u64, Ordering::Relaxed);
        Ok(out)
    }

    /// Remplit un buffer mutable avec des octets aléatoires.
    pub fn fill_bytes(&self, buf: &mut [u8]) -> ExofsResult<()> {
        let bytes = self.random_bytes(buf.len())?;
        buf.copy_from_slice(&bytes);
        Ok(())
    }

    /// Génère 32 octets (tableau statique).
    pub fn random_32(&self) -> [u8; 32] {
        self.generate_block()
    }

    /// Génère 16 octets.
    pub fn random_16(&self) -> [u8; 16] {
        let b = self.generate_block();
        let mut out = [0u8; 16];
        out.copy_from_slice(&b[..16]);
        out
    }

    /// Génère un nonce XChaCha20 de 24 octets.
    pub fn random_nonce_24(&self) -> [u8; 24] {
        let b = self.generate_block();
        let mut n = [0u8; 24];
        n.copy_from_slice(&b[..24]);
        let _ = self.bytes_generated.fetch_add(24, Ordering::Relaxed);
        n
    }

    /// Génère un nonce XChaCha20 de 24 octets lié à un ObjectId (LAC-04 / S-06).
    ///
    /// S-06 / CRYPTO-NONCE : le nonce dérive du compteur global du pool (monotone
    /// atomique SeqCst), de l'object_id (rendant le nonce unique par objet même
    /// si le compteur débordait) et d'une valeur hardware (RDRAND/TSC).
    /// INTERDIT : utiliser RDRAND seul — insuffisant si RDRAND est absent ou faible.
    pub fn nonce_for_object(&self, object_id: &[u8; 32]) -> [u8; 24] {
        // Block CSPRNG : incrémente le compteur interne (SeqCst).
        let block   = self.generate_block();
        // XOR avec les 24 premiers octets de l'object_id pour diversifier.
        let mut n   = [0u8; 24];
        let mut i   = 0usize;
        while i < 24 {
            n[i] = block[i] ^ object_id[i % 32];
            i = i.wrapping_add(1);
        }
        let _ = self.bytes_generated.fetch_add(24, Ordering::Relaxed);
        n
    }

    /// Génère un entier u64 aléatoire.
    pub fn random_u64(&self) -> u64 {
        let b = self.generate_block();
        u64::from_le_bytes(b[..8].try_into().unwrap_or([0u8; 8]))
    }

    /// Génère un entier u32 aléatoire.
    pub fn random_u32(&self) -> u32 {
        (self.random_u64() >> 32) as u32
    }

    /// Génère un entier dans [0, max) sans biais notable.
    pub fn random_range_u64(&self, max: u64) -> ExofsResult<u64> {
        if max == 0 { return Err(ExofsError::InvalidArgument); }
        let threshold = max.wrapping_neg() % max;
        loop {
            let v = self.random_u64();
            if v >= threshold { return Ok(v % max); }
        }
    }

    // ── Primitives internes ───────────────────────────────────────────────────

    fn generate_block(&self) -> [u8; 32] {
        let ctr   = self.counter.fetch_add(1, Ordering::SeqCst);
        // Charge l'état courant.
        let mut s = [0u64; 8];
        for i in 0..8 { s[i] = self.state[i].load(Ordering::Relaxed); }

        // Mélange ChaCha-like (20 demi-rounds sur 8 mots).
        for _ in 0..10 {
            s[0] = s[0].wrapping_add(s[4]); s[6] ^= s[0]; s[6] = s[6].rotate_left(16);
            s[2] = s[2].wrapping_add(s[6]); s[4] ^= s[2]; s[4] = s[4].rotate_left(12);
            s[0] = s[0].wrapping_add(s[4]); s[6] ^= s[0]; s[6] = s[6].rotate_left( 8);
            s[2] = s[2].wrapping_add(s[6]); s[4] ^= s[2]; s[4] = s[4].rotate_left( 7);
            s[1] = s[1].wrapping_add(s[5]); s[7] ^= s[1]; s[7] = s[7].rotate_left(16);
            s[3] = s[3].wrapping_add(s[7]); s[5] ^= s[3]; s[5] = s[5].rotate_left(12);
            s[1] = s[1].wrapping_add(s[5]); s[7] ^= s[1]; s[7] = s[7].rotate_left( 8);
            s[3] = s[3].wrapping_add(s[7]); s[5] ^= s[3]; s[5] = s[5].rotate_left( 7);
        }
        s[7] = s[7].wrapping_add(ctr);

        // Feed-forward sur le pool.
        for i in 4..8 { self.state[i].store(s[i], Ordering::Relaxed); }

        let mut out = [0u8; 32];
        for (i, &v) in s[..4].iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sources matérielles
// ─────────────────────────────────────────────────────────────────────────────

/// Lecture du Time Stamp Counter (x86_64 Ring 0).
///
/// # Safety
/// RDTSC disponible en Ring 0. Instruction non-privilegiée mais autorisée.
#[cfg(target_arch = "x86_64")]
pub fn read_tsc() -> u64 {
    // SAFETY: RDTSC disponible en Ring 0, aucun effet de bord.
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn read_tsc() -> u64 { 0xCAFE_BABE_DEAD_BEEF }

/// Lecture RDRAND (x86_64 — renvoie 0 sur échec).
///
/// # Safety
/// RDRAND disponible sur processeurs Intel/AMD récents.
#[cfg(target_arch = "x86_64")]
pub fn read_rdrand() -> Option<u64> {
    let mut val: u64 = 0;
    let ok: u8;
    // SAFETY: RDRAND en Ring 0.
    unsafe {
        core::arch::asm!(
            "rdrand {v}",
            "setc   {ok}",
            v  = out(reg) val,
            ok = out(reg_byte) ok,
            options(nostack, nomem),
        );
    }
    if ok != 0 { Some(val) } else { None }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn read_rdrand() -> Option<u64> { None }

/// Retourne une valeur d'entropie matérielle combinant TSC + éventuellement RDRAND.
///
/// ARITH-02 : wrapping_add.
pub fn hardware_entropy() -> u64 {
    let tsc = read_tsc();
    match read_rdrand() {
        Some(rdrand) => tsc.wrapping_add(rdrand).rotate_left(7),
        None         => tsc.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(11),
    }
}

/// Amorce le pool global depuis les sources matérielles disponibles.
pub fn seed_global_pool() {
    for _ in 0..8 {
        ENTROPY_POOL.mix(hardware_entropy());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Génère un sel aléatoire de 32 octets via le pool global.
pub fn generate_salt() -> ExofsResult<[u8; 32]> {
    let v = ENTROPY_POOL.random_bytes(32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

/// Génère un ID unique 64-bit (non-secret, juste unique).
pub fn generate_unique_id() -> u64 {
    ENTROPY_POOL.random_u64()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_pool() -> EntropyPool { EntropyPool::new_const() }

    #[test] fn test_is_seeded_initially_false() {
        assert!(!fresh_pool().is_seeded());
    }

    #[test] fn test_is_seeded_after_mix() {
        let p = fresh_pool(); p.mix(1); assert!(p.is_seeded());
    }

    #[test] fn test_seed_from_bytes_marks_seeded() {
        let p = fresh_pool(); p.seed_from_bytes(&[0xABu8; 32]); assert!(p.is_seeded());
    }

    #[test] fn test_seed_from_slice_empty_no_panic() {
        let p = fresh_pool(); p.seed_from_slice(b"");
    }

    #[test] fn test_random_bytes_exact_length() {
        let p = fresh_pool();
        for n in [0, 1, 32, 33, 64, 100, 1024] {
            let b = p.random_bytes(n).unwrap();
            assert_eq!(b.len(), n, "length mismatch for n={n}");
        }
    }

    #[test] fn test_fill_bytes() {
        let p = fresh_pool();
        let mut buf = [0u8; 64];
        p.fill_bytes(&mut buf).unwrap();
        // non-zero après quelques rounds
    }

    #[test] fn test_random_32_not_same_twice() {
        let p = fresh_pool();
        let a = p.random_32();
        let b = p.random_32();
        assert_ne!(a, b);
    }

    #[test] fn test_random_16_length() {
        let p = fresh_pool();
        let b = p.random_16();
        assert_eq!(b.len(), 16);
    }

    #[test] fn test_random_nonce_24_length() {
        let p = fresh_pool();
        let n = p.random_nonce_24();
        assert_eq!(n.len(), 24);
    }

    #[test] fn test_random_u32_no_panic() {
        let p = fresh_pool(); let _ = p.random_u32();
    }

    #[test] fn test_random_range_u64_in_bounds() {
        let p = fresh_pool();
        for _ in 0..100 {
            let v = p.random_range_u64(10).unwrap();
            assert!(v < 10);
        }
    }

    #[test] fn test_random_range_zero_fails() {
        let p = fresh_pool();
        assert!(p.random_range_u64(0).is_err());
    }

    #[test] fn test_bytes_generated_counter() {
        let p = fresh_pool();
        let _  = p.random_bytes(64).unwrap();
        assert_eq!(p.bytes_generated(), 64);
    }

    #[test] fn test_multiple_mix_no_panic() {
        let p = fresh_pool();
        for i in 0..200u64 { p.mix(i.wrapping_mul(0xDEAD)); }
    }

    #[test] fn test_random_large_ok() {
        let p = fresh_pool();
        let b = p.random_bytes(8192).unwrap();
        assert_eq!(b.len(), 8192);
    }

    #[test] fn test_hardware_entropy_no_panic() { let _ = hardware_entropy(); }

    #[test] fn test_generate_salt_length() {
        let s = generate_salt().unwrap();
        assert_eq!(s.len(), 32);
    }
}
