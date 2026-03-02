//! Wrapper LZ4 pour ExoFS (implémentation logicielle embarquée Ring 0).
//!
//! LZ4 Block format — pas de frame format.
//! RÈGLE 1  : no_std uniquement.
//! RÈGLE 2  : try_reserve avant tout push.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;

/// Compresseur LZ4 (block mode).
pub struct Lz4Compressor;

impl Lz4Compressor {
    /// Compresse `input` vers `output`. Retourne la taille compressée.
    ///
    /// Implémentation : LZ4 block codec embarqué (aucune dépendance std).
    pub fn compress(input: &[u8], output: &mut Vec<u8>) -> Result<usize, FsError> {
        if input.is_empty() {
            return Ok(0);
        }
        let bound = lz4_compress_bound(input.len());
        let prev_len = output.len();
        output.try_reserve(bound).map_err(|_| FsError::OutOfMemory)?;
        output.resize(prev_len + bound, 0u8);
        let n = lz4_compress_block(input, &mut output[prev_len..])
            .ok_or(FsError::CompressionFailed)?;
        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Décompresse `input` vers `output` (taille de sortie connue).
    pub fn decompress(
        input: &[u8],
        output: &mut Vec<u8>,
        decompressed_size: usize,
    ) -> Result<usize, FsError> {
        let prev_len = output.len();
        output
            .try_reserve(decompressed_size)
            .map_err(|_| FsError::OutOfMemory)?;
        output.resize(prev_len + decompressed_size, 0u8);
        let n = lz4_decompress_block(input, &mut output[prev_len..])
            .ok_or(FsError::DecompressionFailed)?;
        output.truncate(prev_len + n);
        Ok(n)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Implémentation LZ4 block format embarquée (subset complet pour kernel no_std)
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
    let mut sp: usize = 0; // source position
    let mut dp: usize = 0; // dest position
    let mut anchor = 0usize;

    let src_len = src.len();
    if src_len < MIN_MATCH {
        // Trop court : literal only.
        let lit_len = src_len;
        if dp + lit_len + 1 > dst.len() {
            return None;
        }
        encode_lit_len(&mut dst[dp..], lit_len, &mut dp);
        dst[dp..dp + lit_len].copy_from_slice(src);
        dp += lit_len;
        // EOS match token (0 match len).
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

        // Vérifie un match de MIN_MATCH bytes.
        let is_match = match_pos < sp
            && sp.wrapping_sub(match_pos) <= 65535
            && src.get(match_pos..match_pos + MIN_MATCH) == src.get(sp..sp + MIN_MATCH);

        if is_match {
            // Calcule la longueur du match.
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

            // Écrit token + literals + offset + extra match length.
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

            // Offset little-endian.
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

        // Lit la longueur des literals.
        let mut lit_len = (token >> 4) as usize;
        if lit_len == 15 {
            loop {
                if sp >= src.len() { return None; }
                let extra = src[sp] as usize; sp += 1;
                lit_len = lit_len.checked_add(extra)?;
                if extra != 255 { break; }
            }
        }

        // Copie les literals.
        if dp + lit_len > dst.len() { return None; }
        if sp + lit_len > src.len() { return None; }
        dst[dp..dp + lit_len].copy_from_slice(&src[sp..sp + lit_len]);
        sp += lit_len;
        dp += lit_len;

        if sp >= src.len() { break; } // EOS

        // Lit l'offset du match.
        if sp + 2 > src.len() { return None; }
        let offset = u16::from_le_bytes([src[sp], src[sp + 1]]) as usize;
        sp += 2;
        if offset == 0 { return None; }

        // Lit la longueur du match.
        let mut ml = (token & 0x0F) as usize + 4;
        if (token & 0x0F) == 15 {
            loop {
                if sp >= src.len() { return None; }
                let extra = src[sp] as usize; sp += 1;
                ml = ml.checked_add(extra)?;
                if extra != 255 { break; }
            }
        }

        // Copie le match (overlap possible → octet par octet).
        let match_start = dp.checked_sub(offset)?;
        for i in 0..ml {
            if dp >= dst.len() { return None; }
            dst[dp] = dst[match_start + i];
            dp += 1;
        }
    }

    Some(dp)
}
