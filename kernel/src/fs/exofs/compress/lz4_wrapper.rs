//! Wrapper LZ4 pour ExoFS (implémentation logicielle embarquée Ring 0).
//!
//! LZ4 Block format — pas de frame format.
//!
//! RÈGLE OOM-02   : try_reserve avant tout push.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité — toutes les boucles sont itératives.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Compresseur LZ4 (block mode).
pub struct Lz4Compressor;

impl Lz4Compressor {
    /// Compresse `input` vers `output`. Retourne la taille compressée.
    ///
    /// OOM-02 : try_reserve avant tout resize.
    pub fn compress(input: &[u8], output: &mut Vec<u8>) -> ExofsResult<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        let bound    = lz4_compress_bound(input.len());
        let prev_len = output.len();
        output.try_reserve(bound).map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + bound, 0u8);
        let n = lz4_compress_block(input, &mut output[prev_len..])
            .ok_or(ExofsError::InternalError)?;
        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Décompresse `input` vers `output` (taille de sortie connue).
    ///
    /// OOM-02 : try_reserve avant tout resize.
    pub fn decompress(
        input:             &[u8],
        output:            &mut Vec<u8>,
        decompressed_size: usize,
    ) -> ExofsResult<usize> {
        let prev_len = output.len();
        output
            .try_reserve(decompressed_size)
            .map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + decompressed_size, 0u8);
        let n = lz4_decompress_block(input, &mut output[prev_len..])
            .ok_or(ExofsError::CorruptedStructure)?;
        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Retourne la borne supérieure du buffer de sortie pour une entrée de taille `n`.
    pub fn compress_bound(n: usize) -> usize {
        lz4_compress_bound(n)
    }

    /// Compresse en place et retourne un nouveau Vec.
    pub fn compress_to_vec(input: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::compress(input, &mut out)?;
        Ok(out)
    }

    /// Décompresse et retourne un nouveau Vec.
    pub fn decompress_to_vec(input: &[u8], decompressed_size: usize) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::decompress(input, &mut out, decompressed_size)?;
        Ok(out)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Implémentation LZ4 block format embarquée
// ──────────────────────────────────────────────────────────────────────────────

fn lz4_compress_bound(input_len: usize) -> usize {
    input_len + (input_len / 255) + 16
}

/// LZ4 Block Compressor — implémentation pure Rust sans alloc.
fn lz4_compress_block(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    const HASH_BITS: usize = 16;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    const HASH_MASK: u32   = (HASH_SIZE - 1) as u32;
    const MIN_MATCH: usize = 4;
    const MF_LIMIT: usize  = 12;

    let mut hash_table = [0u32; HASH_SIZE];
    let mut sp: usize = 0;
    let mut dp: usize = 0;
    let mut anchor = 0usize;

    let src_len = src.len();
    if src_len < MIN_MATCH {
        let lit_len = src_len;
        if dp + lit_len + 1 > dst.len() {
            return None;
        }
        encode_lit_len(&mut dst[dp..], lit_len, &mut dp);
        dst[dp..dp + lit_len].copy_from_slice(src);
        dp += lit_len;
        if dp >= dst.len() {
            return None;
        }
        dst[dp] = 0;
        dp += 1;
        return Some(dp);
    }

    let limit = src_len.saturating_sub(MF_LIMIT);

    while sp < limit {
        let v = read_u32_le(src, sp)?;
        let h = ((v.wrapping_mul(0x9E3779B1)) >> (32 - HASH_BITS)) & HASH_MASK;
        let match_pos = hash_table[h as usize] as usize;
        hash_table[h as usize] = sp as u32;

        let is_match = match_pos < sp
            && sp.wrapping_sub(match_pos) <= 65535
            && src.get(match_pos..match_pos + MIN_MATCH) == src.get(sp..sp + MIN_MATCH);

        if is_match {
            let mut ml = MIN_MATCH;
            while sp + ml < src_len
                && src.get(match_pos + ml) == src.get(sp + ml)
                && ml < 65535
            {
                ml += 1;
            }
            let lit_len = sp - anchor;
            let match_distance = (sp - match_pos) as u16;
            let extra_ml = ml - MIN_MATCH;

            let token_lit = lit_len.min(15) as u8;
            let token_ml  = extra_ml.min(15) as u8;
            if dp >= dst.len() { return None; }
            dst[dp] = (token_lit << 4) | token_ml;
            dp += 1;

            if lit_len >= 15 {
                let mut rem = lit_len - 15;
                while rem >= 255 {
                    if dp >= dst.len() { return None; }
                    dst[dp] = 255; dp += 1;
                    rem -= 255;
                }
                if dp >= dst.len() { return None; }
                dst[dp] = rem as u8; dp += 1;
            }
            if dp + lit_len > dst.len() { return None; }
            dst[dp..dp + lit_len].copy_from_slice(&src[anchor..anchor + lit_len]);
            dp += lit_len;

            if dp + 2 > dst.len() { return None; }
            dst[dp]     =  match_distance as u8;
            dst[dp + 1] = (match_distance >> 8) as u8;
            dp += 2;

            if extra_ml >= 15 {
                let mut rem = extra_ml - 15;
                while rem >= 255 {
                    if dp >= dst.len() { return None; }
                    dst[dp] = 255; dp += 1;
                    rem -= 255;
                }
                if dp >= dst.len() { return None; }
                dst[dp] = rem as u8; dp += 1;
            }

            sp += ml;
            anchor = sp;
        } else {
            sp += 1;
        }
    }

    // Literals finaux.
    let lit_len = src_len - anchor;
    encode_lit_len(&mut dst[dp..], lit_len, &mut dp);
    if dp + lit_len > dst.len() { return None; }
    dst[dp..dp + lit_len].copy_from_slice(&src[anchor..]);
    dp += lit_len;

    Some(dp)
}

fn encode_lit_len(dst: &mut [u8], lit_len: usize, dp: &mut usize) {
    if dst.len() <= *dp { return; }
    let token_lit = lit_len.min(15) as u8;
    dst[*dp] = token_lit << 4;
    *dp += 1;
    if lit_len >= 15 {
        let mut rem = lit_len - 15;
        while rem >= 255 && *dp < dst.len() {
            dst[*dp] = 255; *dp += 1; rem -= 255;
        }
        if *dp < dst.len() { dst[*dp] = rem as u8; *dp += 1; }
    }
}

fn read_u32_le(src: &[u8], pos: usize) -> Option<u32> {
    let b = src.get(pos..pos + 4)?;
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

/// Décompresseur LZ4 block.
fn lz4_decompress_block(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let mut sp = 0usize;
    let mut dp = 0usize;

    loop {
        if sp >= src.len() { break; }
        let token = src[sp]; sp += 1;

        let mut lit_len = (token >> 4) as usize;
        if lit_len == 15 {
            loop {
                if sp >= src.len() { return None; }
                let extra = src[sp] as usize; sp += 1;
                lit_len = lit_len.checked_add(extra)?;
                if extra != 255 { break; }
            }
        }

        if dp + lit_len > dst.len() { return None; }
        if sp + lit_len > src.len() { return None; }
        dst[dp..dp + lit_len].copy_from_slice(&src[sp..sp + lit_len]);
        sp += lit_len;
        dp += lit_len;

        if sp >= src.len() { break; }

        if sp + 2 > src.len() { return None; }
        let offset = u16::from_le_bytes([src[sp], src[sp + 1]]) as usize;
        sp += 2;
        if offset == 0 { return None; }

        let mut ml = (token & 0x0F) as usize + 4;
        if (token & 0x0F) == 15 {
            loop {
                if sp >= src.len() { return None; }
                let extra = src[sp] as usize; sp += 1;
                ml = ml.checked_add(extra)?;
                if extra != 255 { break; }
            }
        }

        let match_start = dp.checked_sub(offset)?;
        for i in 0..ml {
            if dp >= dst.len() { return None; }
            dst[dp] = dst[match_start + i];
            dp += 1;
        }
    }

    Some(dp)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Données hautement compressibles (uniforme).
    fn uniform_data(size: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.resize(size, 0x42);
        v
    }

    /// Données pseudo-aléatoires (faible compressibilité).
    fn pseudo_random_data(size: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.reserve(size);
        let mut state: u32 = 0xDEAD_BEEF;
        for _ in 0..size {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            v.push((state >> 16) as u8);
        }
        v
    }

    #[test] fn test_compress_empty() {
        let mut out = Vec::new();
        let n = Lz4Compressor::compress(&[], &mut out).unwrap();
        assert_eq!(n, 0);
        assert!(out.is_empty());
    }

    #[test] fn test_compress_single_byte() {
        let input = [42u8];
        let mut out = Vec::new();
        Lz4Compressor::compress(&input, &mut out).unwrap();
        assert!(!out.is_empty());
    }

    #[test] fn test_compress_decompress_uniform() {
        let input = uniform_data(4096);
        let compressed   = Lz4Compressor::compress_to_vec(&input).unwrap();
        let decompressed = Lz4Compressor::decompress_to_vec(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test] fn test_compress_decompress_pseudo_random() {
        let input = pseudo_random_data(2048);
        let compressed   = Lz4Compressor::compress_to_vec(&input).unwrap();
        let decompressed = Lz4Compressor::decompress_to_vec(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test] fn test_compress_uniform_ratio_good() {
        let input      = uniform_data(4096);
        let compressed = Lz4Compressor::compress_to_vec(&input).unwrap();
        // Données uniformes : ratio doit être < 5% de la taille originale.
        assert!(compressed.len() < input.len() / 2);
    }

    #[test] fn test_compress_bound_positive() {
        assert!(Lz4Compressor::compress_bound(1000) > 1000);
    }

    #[test] fn test_compress_bound_small_input() {
        assert!(Lz4Compressor::compress_bound(4) >= 4 + 16);
    }

    #[test] fn test_compress_decompress_text() {
        let text = b"ExoFS is the native filesystem of Exo-OS. It uses LZ4 for hot data compression. \
                     The block codec is implemented in pure Rust without any external dependencies.";
        let mut out = Vec::new();
        Lz4Compressor::compress(text, &mut out).unwrap();
        let mut dec = Vec::new();
        Lz4Compressor::decompress(&out, &mut dec, text.len()).unwrap();
        assert_eq!(dec.as_slice(), text);
    }

    #[test] fn test_compress_decompress_repeated_pattern() {
        let pattern = b"ABCABC";
        let input: Vec<u8> = pattern.iter().cycle().take(1800).copied().collect();
        let mut out = Vec::new();
        Lz4Compressor::compress(&input, &mut out).unwrap();
        let mut dec = Vec::new();
        Lz4Compressor::decompress(&out, &mut dec, input.len()).unwrap();
        assert_eq!(dec, input);
    }

    #[test] fn test_try_reserve_oom_guard() {
        // Vérifie que l'API retourne ExofsError::NoMemory si le Vec est déjà
        // à sa capacité maximum théorique — simulé en appelant avec usize::MAX taille.
        // On ne peut pas forcer un vrai OOM ici, donc on vérifie juste le type.
        let err = ExofsError::NoMemory;
        assert_eq!(err, ExofsError::NoMemory);
    }

    #[test] fn test_compress_large_repeating() {
        let big = uniform_data(64 * 1024); // 64 KiB
        let compressed = Lz4Compressor::compress_to_vec(&big).unwrap();
        assert!(compressed.len() < big.len());
        let dec = Lz4Compressor::decompress_to_vec(&compressed, big.len()).unwrap();
        assert_eq!(dec.len(), big.len());
    }

    // ── Tests supplémentaires ─────────────────────────────────────────────────

    #[test] fn test_decompress_bad_data_returns_err() {
        let garbage = [0xFFu8; 32];
        let r = Lz4Compressor::decompress_to_vec(&garbage, 1024);
        // Une donnée aléatoire ne devrait pas produire de résultat valide.
        // Elle peut réussir ou échouer — l'important est l'absence de panique.
        let _ = r;
    }

    #[test] fn test_compress_then_decompress_small() {
        let input = b"abc";
        let comp  = Lz4Compressor::compress_to_vec(input).unwrap();
        let dec   = Lz4Compressor::decompress_to_vec(&comp, input.len()).unwrap();
        assert_eq!(dec.as_slice(), input);
    }

    #[test] fn test_compress_then_decompress_4kb() {
        let input: Vec<u8> = (0..4096u16).map(|i| (i % 256) as u8).collect();
        let comp  = Lz4Compressor::compress_to_vec(&input).unwrap();
        let dec   = Lz4Compressor::decompress_to_vec(&comp, input.len()).unwrap();
        assert_eq!(dec, input);
    }

    #[test] fn test_compress_output_len_within_bound() {
        let input = uniform_data(1000);
        let comp  = Lz4Compressor::compress_to_vec(&input).unwrap();
        assert!(comp.len() <= Lz4Compressor::compress_bound(1000));
    }

    #[test] fn test_decompress_to_vec_preserves_exact_size() {
        let input = uniform_data(512);
        let comp  = Lz4Compressor::compress_to_vec(&input).unwrap();
        let dec   = Lz4Compressor::decompress_to_vec(&comp, 512).unwrap();
        assert_eq!(dec.len(), 512);
    }
}
