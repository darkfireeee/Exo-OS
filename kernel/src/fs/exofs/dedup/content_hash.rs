//! ContentHash — calcul d'empreintes de contenu pour la déduplication (no_std).
//!
//! RÈGLE 11 : le BlobId d'un objet = Blake3(données AVANT compression/chiffrement).
//! Le ContentHash est identique au BlobId pour les données brutes.
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>

use crate::fs::exofs::core::{BlobId, FsError};
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Algorithme de hachage de contenu.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgorithm {
    Blake3   = 0,
    Xxhash64 = 1,   // Hachage rapide non-cryptographique pour la détection rapide.
}

/// Résultat d'un hachage de contenu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentHashResult {
    pub blake3: [u8; 32],
    pub xxhash: u64,
}

impl ContentHashResult {
    pub fn blob_id(&self) -> BlobId {
        BlobId::from_raw(self.blake3)
    }
}

/// Registre global de hashes de contenu connus.
pub static CONTENT_HASH: ContentHash = ContentHash::new_const();

pub struct ContentHash {
    cache:          SpinLock<BTreeMap<BlobId, ContentHashResult>>,
    computations:   AtomicU64,
    cache_hits:     AtomicU64,
}

impl ContentHash {
    pub const fn new_const() -> Self {
        Self {
            cache:        SpinLock::new(BTreeMap::new()),
            computations: AtomicU64::new(0),
            cache_hits:   AtomicU64::new(0),
        }
    }

    /// Calcule le ContentHash d'un bloc de données.
    pub fn compute(data: &[u8]) -> ContentHashResult {
        let blake3 = blake3_hash(data);
        let xxhash = xxhash64(data, 0);
        ContentHashResult { blake3, xxhash }
    }

    /// Calcule et met en cache le ContentHash.
    pub fn compute_and_cache(&self, data: &[u8]) -> Result<ContentHashResult, FsError> {
        let result = Self::compute(data);
        let bid = result.blob_id();
        {
            let mut cache = self.cache.lock();
            if !cache.contains_key(&bid) {
                cache.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                cache.insert(bid, result);
            }
        }
        self.computations.fetch_add(1, Ordering::Relaxed);
        Ok(result)
    }

    /// Consulte le cache par BlobId.
    pub fn get_cached(&self, blob_id: &BlobId) -> Option<ContentHashResult> {
        let cache = self.cache.lock();
        cache.get(blob_id).copied()
    }

    /// Vérifie si un blob est déjà connu.
    pub fn is_known(&self, blob_id: &BlobId) -> bool {
        self.cache.lock().contains_key(blob_id)
    }

    /// Supprime une entrée du cache.
    pub fn evict(&self, blob_id: &BlobId) {
        self.cache.lock().remove(blob_id);
    }

    pub fn computations(&self) -> u64 { self.computations.load(Ordering::Relaxed) }
    pub fn cache_hits(&self) -> u64 { self.cache_hits.load(Ordering::Relaxed) }
    pub fn cache_size(&self) -> usize { self.cache.lock().len() }
}

// ──────────────────────────────────────────────────────────────────────────────
// Primitives de hachage
// ──────────────────────────────────────────────────────────────────────────────

fn blake3_hash(data: &[u8]) -> [u8; 32] {
    // Implémentation single-chunk Blake3 (≤ 1024 bytes par chunk, mode root).
    const IV: [u32; 8] = [
        0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
        0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
    ];
    const BLOCK_SIZE: usize = 64;
    const FLAG_CHUNK_START: u32 = 1;
    const FLAG_CHUNK_END:   u32 = 2;
    const FLAG_ROOT:        u32 = 8;

    let mut chaining_value = IV;
    let mut offset:         u64 = 0;

    let chunks: Vec<&[u8]> = data.chunks(1024).collect();
    let n_chunks = chunks.len().max(1);

    for (ci, chunk) in data.chunks(1024).enumerate() {
        let is_first = ci == 0;
        let is_last  = ci == n_chunks - 1;
        let mut cv = if is_first { IV } else { chaining_value };

        for (bi, block) in chunk.chunks(BLOCK_SIZE).enumerate() {
            let is_block_first = bi == 0;
            let is_block_last  = bi == (chunk.len() + BLOCK_SIZE - 1) / BLOCK_SIZE - 1;
            let mut msg = [0u8; 64];
            msg[..block.len()].copy_from_slice(block);
            let words = bytes_to_words_64(&msg);
            let mut flags: u32 = 0;
            if is_first && is_block_first { flags |= FLAG_CHUNK_START; }
            if is_last  && is_block_last  { flags |= FLAG_CHUNK_END; }
            if is_last  && is_block_last  { flags |= FLAG_ROOT; }
            let result = blake3_compress(&cv, &words, offset, block.len() as u32, flags);
            cv[0] = result[0]; cv[1] = result[1]; cv[2] = result[2]; cv[3] = result[3];
            cv[4] = result[4]; cv[5] = result[5]; cv[6] = result[6]; cv[7] = result[7];
        }
        chaining_value = cv;
        offset = offset.wrapping_add(1024);
    }

    let mut out = [0u8; 32];
    for (i, &w) in chaining_value.iter().enumerate() {
        out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn bytes_to_words_64(b: &[u8; 64]) -> [u32; 16] {
    let mut w = [0u32; 16];
    for i in 0..16 {
        w[i] = u32::from_le_bytes(b[i*4..i*4+4].try_into().unwrap());
    }
    w
}

fn blake3_compress(cv: &[u32; 8], msg: &[u32; 16], ctr: u64, blen: u32, flags: u32) -> [u32; 16] {
    let iv = [0x6A09E667u32, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A];
    let mut v: [u32; 16] = [
        cv[0],cv[1],cv[2],cv[3],cv[4],cv[5],cv[6],cv[7],
        iv[0],iv[1],iv[2],iv[3],
        (ctr&0xFFFF_FFFF) as u32,(ctr>>32) as u32,blen,flags,
    ];
    const SIGMA:[[usize;16];7]=[
        [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15],
        [2,6,3,10,7,0,4,13,1,11,12,5,9,14,15,8],
        [3,4,10,12,13,2,7,14,6,5,9,0,11,15,8,1],
        [10,7,12,9,14,3,13,15,4,0,11,2,5,8,1,6],
        [12,13,9,11,15,10,14,8,7,2,5,4,0,1,6,3],
        [14,15,11,5,8,12,9,2,13,3,0,7,1,6,4,10],
        [14,15,11,5,8,12,9,2,13,3,0,7,1,6,4,10],
    ];
    let mut g=|a:usize,b:usize,c:usize,d:usize,x:u32,y:u32|{
        v[a]=v[a].wrapping_add(v[b]).wrapping_add(x);v[d]=(v[d]^v[a]).rotate_right(16);
        v[c]=v[c].wrapping_add(v[d]);v[b]=(v[b]^v[c]).rotate_right(12);
        v[a]=v[a].wrapping_add(v[b]).wrapping_add(y);v[d]=(v[d]^v[a]).rotate_right(8);
        v[c]=v[c].wrapping_add(v[d]);v[b]=(v[b]^v[c]).rotate_right(7);
    };
    for r in 0..7{let s=&SIGMA[r];
        g(0,4,8,12,msg[s[0]],msg[s[1]]);g(1,5,9,13,msg[s[2]],msg[s[3]]);
        g(2,6,10,14,msg[s[4]],msg[s[5]]);g(3,7,11,15,msg[s[6]],msg[s[7]]);
        g(0,5,10,15,msg[s[8]],msg[s[9]]);g(1,6,11,12,msg[s[10]],msg[s[11]]);
        g(2,7,8,13,msg[s[12]],msg[s[13]]);g(3,4,9,14,msg[s[14]],msg[s[15]]);}
    for i in 0..8{v[i]^=v[i+8];v[i+8]^=cv[i];}
    v
}

/// xxHash-64 non-cryptographique (Knuth multiplier variant simplifié).
fn xxhash64(data: &[u8], seed: u64) -> u64 {
    const P1: u64 = 0x9E3779B185EBCA87;
    const P2: u64 = 0xC2B2AE3D27D4EB4F;
    const P3: u64 = 0x165667B19E3779F9;
    const P4: u64 = 0x85EBCA77C2B2AE63;
    const P5: u64 = 0x27D4EB2F165667C5;

    let len = data.len();
    let mut h: u64;

    if len >= 32 {
        let mut v1 = seed.wrapping_add(P1).wrapping_add(P2);
        let mut v2 = seed.wrapping_add(P2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(P1);
        let mut p = 0usize;

        while p + 32 <= len {
            let lane = |off: usize| u64::from_le_bytes(data[p+off..p+off+8].try_into().unwrap());
            v1 = v1.wrapping_add(lane(0).wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
            v2 = v2.wrapping_add(lane(8).wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
            v3 = v3.wrapping_add(lane(16).wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
            v4 = v4.wrapping_add(lane(24).wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
            p += 32;
        }

        h = v1.rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));

        h = (h ^ (v1.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1))).wrapping_mul(P1).wrapping_add(P4);
        h = (h ^ (v2.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1))).wrapping_mul(P1).wrapping_add(P4);
        h = (h ^ (v3.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1))).wrapping_mul(P1).wrapping_add(P4);
        h = (h ^ (v4.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1))).wrapping_mul(P1).wrapping_add(P4);
        h = h.wrapping_add(len as u64);

        let mut p2 = p;
        while p2 + 8 <= len {
            let lane = u64::from_le_bytes(data[p2..p2+8].try_into().unwrap());
            h ^= lane.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1);
            h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
            p2 += 8;
        }
        if p2 + 4 <= len {
            let word = u32::from_le_bytes(data[p2..p2+4].try_into().unwrap()) as u64;
            h ^= word.wrapping_mul(P1);
            h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
            p2 += 4;
        }
        for &b in &data[p2..] {
            h ^= (b as u64).wrapping_mul(P5);
            h = h.rotate_left(11).wrapping_mul(P1);
        }
    } else {
        h = seed.wrapping_add(P5).wrapping_add(len as u64);
        let mut p = 0usize;
        while p + 8 <= len {
            let lane = u64::from_le_bytes(data[p..p+8].try_into().unwrap());
            h ^= lane.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1);
            h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
            p += 8;
        }
        if p + 4 <= len {
            let word = u32::from_le_bytes(data[p..p+4].try_into().unwrap()) as u64;
            h ^= word.wrapping_mul(P1);
            h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
            p += 4;
        }
        for &b in &data[p..] {
            h ^= (b as u64).wrapping_mul(P5);
            h = h.rotate_left(11).wrapping_mul(P1);
        }
    }

    h ^= h >> 33;
    h  = h.wrapping_mul(P2);
    h ^= h >> 29;
    h  = h.wrapping_mul(P3);
    h ^= h >> 32;
    h
}
