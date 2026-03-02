//! EntropyPool — génération d'aléa cryptographique pour ExoFS (no_std).
//!
//! Sources : RDRAND x86, compteurs de ticks CPU, mélange ChaCha20-based PRNG.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;

/// Taille de l'état interne du pool (32 bytes = sous-clé ChaCha20).
const POOL_STATE_SIZE: usize = 32;
/// Nombre maximum de bytes délivrés avant re-seeding.
const RESEED_THRESHOLD: u64 = 1 << 20; // 1 MiB.

/// Pool d'entropie global ExoFS.
pub static ENTROPY_POOL: EntropyPool = EntropyPool::new_const();

/// Pool d'entropie — ChaCha20-DRBG interne.
pub struct EntropyPool {
    state: SpinLock<PoolState>,
    bytes_generated: AtomicU64,
}

struct PoolState {
    key: [u8; 32],
    counter: u64,
    seeded: bool,
}

impl EntropyPool {
    pub const fn new_const() -> Self {
        Self {
            state: SpinLock::new(PoolState {
                key: [0u8; 32],
                counter: 0,
                seeded: false,
            }),
            bytes_generated: AtomicU64::new(0),
        }
    }

    /// Initialise le pool depuis les sources matérielles disponibles.
    pub fn seed(&self) {
        let mut s = self.state.lock();
        let mut seed_material = [0u8; 64];

        // Source 1 : RDRAND x86_64.
        for chunk in seed_material[..32].chunks_mut(8) {
            let val = rdrand_u64_safe();
            chunk.copy_from_slice(&val.to_le_bytes());
        }

        // Source 2 : compteurs de ticks.
        let ticks = crate::arch::time::read_ticks();
        seed_material[32..40].copy_from_slice(&ticks.to_le_bytes());

        // Source 3 : compteur interne (anti-replay).
        let cnt = self.bytes_generated.fetch_add(1, Ordering::SeqCst);
        seed_material[40..48].copy_from_slice(&cnt.to_le_bytes());

        // Mélange par Blake3.
        let mixed = blake3_hash(&seed_material);
        s.key.copy_from_slice(&mixed[..32]);
        s.counter = ticks;
        s.seeded = true;
    }

    /// Remplit `out` avec des bytes pseudo-aléatoires cryptographiquement forts.
    pub fn fill_bytes(&self, out: &mut [u8]) {
        {
            let s = self.state.lock();
            if !s.seeded {
                drop(s);
                self.seed();
            }
        }

        let total = out.len();
        let prev = self.bytes_generated.fetch_add(total as u64, Ordering::Relaxed);
        if prev > RESEED_THRESHOLD {
            self.bytes_generated.store(0, Ordering::Relaxed);
            self.reseed_internal();
        }

        let mut s = self.state.lock();
        let mut offset = 0;
        while offset < total {
            let block = chacha20_block_entropy(&s.key, s.counter);
            s.counter = s.counter.wrapping_add(1);
            let n = (total - offset).min(64);
            out[offset..offset + n].copy_from_slice(&block[..n]);
            offset += n;
        }
        // Forward secrecy : rekey depuis le bloc suivant.
        let rekey_block = chacha20_block_entropy(&s.key, s.counter);
        s.key.copy_from_slice(&rekey_block[..32]);
        s.counter = s.counter.wrapping_add(1);
    }

    fn reseed_internal(&self) {
        let extra_tick = crate::arch::time::read_ticks();
        let extra_rdrand = rdrand_u64_safe();
        let mut s = self.state.lock();
        // XOR la clé courante avec du matériel supplémentaire.
        let t = extra_tick.to_le_bytes();
        let r = extra_rdrand.to_le_bytes();
        for i in 0..8  { s.key[i]      ^= t[i]; }
        for i in 0..8  { s.key[8+i]    ^= r[i]; }
        let rekey = chacha20_block_entropy(&s.key, s.counter);
        s.key.copy_from_slice(&rekey[..32]);
        s.counter = s.counter.wrapping_add(1);
    }

    /// Génère `n` bytes dans un Vec alloué.
    pub fn random_bytes(&self, n: usize) -> Result<Vec<u8>, crate::fs::exofs::core::FsError> {
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| crate::fs::exofs::core::FsError::OutOfMemory)?;
        v.resize(n, 0u8);
        self.fill_bytes(&mut v);
        Ok(v)
    }

    /// Génère une clé 256-bit.
    pub fn random_key_256(&self) -> [u8; 32] {
        let mut k = [0u8; 32];
        self.fill_bytes(&mut k);
        k
    }

    /// Génère un nonce XChaCha20 (24 bytes).
    pub fn random_nonce(&self) -> super::xchacha20::Nonce {
        let mut n = [0u8; 24];
        self.fill_bytes(&mut n);
        super::xchacha20::Nonce(n)
    }

    /// Mélange de l'entropie externe (collecte d'événements du kernel).
    pub fn mix_entropy(&self, data: &[u8]) {
        let mut s = self.state.lock();
        // Absorbe les données via Blake3 mélangé à la clé courante.
        let mut to_hash = [0u8; 64];
        to_hash[..32].copy_from_slice(&s.key);
        let n = data.len().min(32);
        to_hash[32..32 + n].copy_from_slice(&data[..n]);
        let mixed = blake3_hash(&to_hash);
        s.key.copy_from_slice(&mixed[..32]);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Primitives internes
// ──────────────────────────────────────────────────────────────────────────────

/// Lit une valeur RDRAND x86_64. Retourne 0 si RDRAND non disponible.
fn rdrand_u64_safe() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // SAFETY: RDRAND est une instruction x86_64 valide sur les CPUs modernes.
        // On vérifie le CF flag pour détecter l'échec.
        let mut val: u64;
        let mut ok: u8;
        core::arch::asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) val,
            ok  = out(reg_byte) ok,
            options(nostack, nomem)
        );
        if ok != 0 { val } else { crate::arch::time::read_ticks() }
    }
    #[cfg(not(target_arch = "x86_64"))]
    { crate::arch::time::read_ticks() }
}

fn chacha20_block_entropy(key: &[u8; 32], counter: u64) -> [u8; 64] {
    let nonce = [0u8; 12]; // Nonce nul pour DRBG interne.
    // Contre valeur 32-bit uniquement (ChaCha20 counter = u32).
    let ctr32 = (counter & 0xFFFF_FFFF) as u32;
    // On réutilise la logique dans xchacha20.
    use super::xchacha20::*;
    // On écrit la logique directement ici pour éviter la circularité.
    low_level_chacha20_block(key, &nonce, ctr32)
}

/// Re-export du bloc ChaCha20 bas-niveau depuis xchacha20 — on le duplique ici.
fn low_level_chacha20_block_inner(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    fn qr(a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32) {
        *a = a.wrapping_add(*b); *d ^= *a; *d = d.rotate_left(16);
        *c = c.wrapping_add(*d); *b ^= *c; *b = b.rotate_left(12);
        *a = a.wrapping_add(*b); *d ^= *a; *d = d.rotate_left(8);
        *c = c.wrapping_add(*d); *b ^= *c; *b = b.rotate_left(7);
    }

    let mut s: [u32; 16] = [
        0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574,
        u32::from_le_bytes(key[0..4].try_into().unwrap()),
        u32::from_le_bytes(key[4..8].try_into().unwrap()),
        u32::from_le_bytes(key[8..12].try_into().unwrap()),
        u32::from_le_bytes(key[12..16].try_into().unwrap()),
        u32::from_le_bytes(key[16..20].try_into().unwrap()),
        u32::from_le_bytes(key[20..24].try_into().unwrap()),
        u32::from_le_bytes(key[24..28].try_into().unwrap()),
        u32::from_le_bytes(key[28..32].try_into().unwrap()),
        counter,
        u32::from_le_bytes(nonce[0..4].try_into().unwrap()),
        u32::from_le_bytes(nonce[4..8].try_into().unwrap()),
        u32::from_le_bytes(nonce[8..12].try_into().unwrap()),
    ];
    let init = s;
    for _ in 0..10 {
        qr(&mut s[0], &mut s[4], &mut s[8],  &mut s[12]);
        qr(&mut s[1], &mut s[5], &mut s[9],  &mut s[13]);
        qr(&mut s[2], &mut s[6], &mut s[10], &mut s[14]);
        qr(&mut s[3], &mut s[7], &mut s[11], &mut s[15]);
        qr(&mut s[0], &mut s[5], &mut s[10], &mut s[15]);
        qr(&mut s[1], &mut s[6], &mut s[11], &mut s[12]);
        qr(&mut s[2], &mut s[7], &mut s[8],  &mut s[13]);
        qr(&mut s[3], &mut s[4], &mut s[9],  &mut s[14]);
    }
    for i in 0..16 { s[i] = s[i].wrapping_add(init[i]); }
    let mut out = [0u8; 64];
    for (i, w) in s.iter().enumerate() {
        out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Retourne 64 bytes ChaCha20 pour l'usage interne DRBG.
pub(super) fn low_level_chacha20_block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    low_level_chacha20_block_inner(key, nonce, counter)
}

/// Blake3 simplifié (compression 256-bit) pour le mixing d'entropie.
fn blake3_hash(input: &[u8]) -> [u8; 32] {
    // Implémentation simplifiée : Blake3 chunk complet (1er chunk uniquement).
    const IV: [u32; 8] = [
        0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
        0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
    ];
    const FLAGS_CHUNK_START: u32 = 1 << 0;
    const FLAGS_CHUNK_END:   u32 = 1 << 1;
    const FLAGS_ROOT:        u32 = 1 << 3;

    let mut msg = [0u8; 64];
    let n = input.len().min(64);
    msg[..n].copy_from_slice(&input[..n]);

    let words: [u32; 16] = {
        let mut w = [0u32; 16];
        for i in 0..16 {
            w[i] = u32::from_le_bytes(msg[i*4..i*4+4].try_into().unwrap());
        }
        w
    };

    let flags = FLAGS_CHUNK_START | FLAGS_CHUNK_END | FLAGS_ROOT;
    let cv = blake3_compress(&IV, &words, 0, n as u32, flags);

    let mut out = [0u8; 32];
    for (i, &w) in cv.iter().take(8).enumerate() {
        out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn blake3_compress(
    chaining_value: &[u32; 8],
    msg: &[u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u32; 16] {
    let iv: [u32; 4] = [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A];
    let mut v: [u32; 16] = [
        chaining_value[0], chaining_value[1], chaining_value[2], chaining_value[3],
        chaining_value[4], chaining_value[5], chaining_value[6], chaining_value[7],
        iv[0], iv[1], iv[2], iv[3],
        (counter & 0xFFFF_FFFF) as u32,
        (counter >> 32) as u32,
        block_len,
        flags,
    ];
    const SIGMA: [[usize; 16]; 7] = [
        [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15],
        [2,6,3,10,7,0,4,13,1,11,12,5,9,14,15,8],
        [3,4,10,12,13,2,7,14,6,5,9,0,11,15,8,1],
        [10,7,12,9,14,3,13,15,4,0,11,2,5,8,1,6],
        [12,13,9,11,15,10,14,8,7,2,5,4,0,1,6,3],
        [14,15,11,5,8,12,9,2,13,3,0,7,1,6,4,10],
        [14,15,11,5,8,12,9,2,13,3,0,7,1,6,4,10],
    ];

    let mut g = |a: usize, b: usize, c: usize, d: usize, x: u32, y: u32| {
        v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
        v[d] = (v[d] ^ v[a]).rotate_right(16);
        v[c] = v[c].wrapping_add(v[d]);
        v[b] = (v[b] ^ v[c]).rotate_right(12);
        v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
        v[d] = (v[d] ^ v[a]).rotate_right(8);
        v[c] = v[c].wrapping_add(v[d]);
        v[b] = (v[b] ^ v[c]).rotate_right(7);
    };

    for round in 0..7 {
        let s = &SIGMA[round];
        g(0, 4,  8, 12, msg[s[0]], msg[s[1]]);
        g(1, 5,  9, 13, msg[s[2]], msg[s[3]]);
        g(2, 6, 10, 14, msg[s[4]], msg[s[5]]);
        g(3, 7, 11, 15, msg[s[6]], msg[s[7]]);
        g(0, 5, 10, 15, msg[s[8]], msg[s[9]]);
        g(1, 6, 11, 12, msg[s[10]], msg[s[11]]);
        g(2, 7,  8, 13, msg[s[12]], msg[s[13]]);
        g(3, 4,  9, 14, msg[s[14]], msg[s[15]]);
    }

    for i in 0..8 { v[i] ^= v[i + 8]; v[i + 8] ^= chaining_value[i]; }
    v
}
