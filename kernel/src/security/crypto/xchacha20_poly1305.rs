// kernel/src/security/crypto/xchacha20_poly1305.rs
//
// XChaCha20-BLAKE3 AEAD pour le kernel.
//
// Le nom de module historique est conservé pour compatibilité API, mais
// l'authentification côté noyau est réalisée avec BLAKE3 keyed-hash plutôt que
// Poly1305 afin d'éviter les dépendances SIMD/SSE2 indisponibles sur
// `x86_64-unknown-none`.

use super::blake3::{blake3_derive_key, constant_time_eq, Blake3Hasher};

/// Longueur du tag d'authentification (16 octets tronqués).
pub const TAG_LEN: usize = 16;
/// Longueur du nonce XChaCha20 (24 octets).
pub const XCHACHA20_NONCE_LEN: usize = 24;
/// Longueur de la clé ChaCha20 (32 octets).
pub const KEY_LEN: usize = 32;

const CHACHA20_BLOCK_SIZE: usize = 64;
const MAC_CONTEXT: &[u8] = b"ExoOS-Kernel-XChaCha20-BLAKE3-MAC-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadError {
    AuthenticationFailed,
    InvalidParameter,
    BufferTooSmall,
    NotAvailableOnThisTarget,
}

impl core::fmt::Display for AeadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AeadError::AuthenticationFailed => {
                write!(f, "XChaCha20-BLAKE3: authentication failed")
            }
            AeadError::InvalidParameter => write!(f, "XChaCha20-BLAKE3: invalid parameter"),
            AeadError::BufferTooSmall => write!(f, "XChaCha20-BLAKE3: output buffer too small"),
            AeadError::NotAvailableOnThisTarget => {
                write!(f, "XChaCha20-BLAKE3: unavailable on this target")
            }
        }
    }
}

#[inline]
pub fn xchacha20_poly1305_seal(
    key: &[u8; KEY_LEN],
    nonce: &[u8; XCHACHA20_NONCE_LEN],
    plaintext: &mut [u8],
    aad: &[u8],
    tag_out: &mut [u8; TAG_LEN],
) -> Result<(), AeadError> {
    xchacha20_apply(key, nonce, plaintext);
    *tag_out = compute_tag(key, nonce, aad, plaintext);
    Ok(())
}

#[inline]
pub fn xchacha20_poly1305_open(
    key: &[u8; KEY_LEN],
    nonce: &[u8; XCHACHA20_NONCE_LEN],
    ciphertext: &mut [u8],
    aad: &[u8],
    tag: &[u8; TAG_LEN],
) -> Result<(), AeadError> {
    let expected = compute_tag(key, nonce, aad, ciphertext);
    if !constant_time_eq(tag, &expected) {
        return Err(AeadError::AuthenticationFailed);
    }
    xchacha20_apply(key, nonce, ciphertext);
    Ok(())
}

fn compute_tag(
    key: &[u8; KEY_LEN],
    nonce: &[u8; XCHACHA20_NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> [u8; TAG_LEN] {
    let mut ikm = [0u8; KEY_LEN + XCHACHA20_NONCE_LEN];
    ikm[..KEY_LEN].copy_from_slice(key);
    ikm[KEY_LEN..].copy_from_slice(nonce);

    let mut mac_key = [0u8; 32];
    blake3_derive_key(MAC_CONTEXT, &ikm, &mut mac_key);

    let mut hasher = Blake3Hasher::new_keyed(&mac_key);
    hasher
        .update(&(aad.len() as u64).to_le_bytes())
        .update(aad)
        .update(&(ciphertext.len() as u64).to_le_bytes())
        .update(ciphertext);

    let mut full_tag = [0u8; 32];
    hasher.finalize(&mut full_tag);

    let mut out = [0u8; TAG_LEN];
    out.copy_from_slice(&full_tag[..TAG_LEN]);
    out
}

fn xchacha20_apply(
    key: &[u8; KEY_LEN],
    nonce: &[u8; XCHACHA20_NONCE_LEN],
    data: &mut [u8],
) {
    let subkey = hchacha20(key, array_ref_16(&nonce[..16]));
    let mut chacha_nonce = [0u8; 12];
    chacha_nonce[4..].copy_from_slice(&nonce[16..]);

    let mut counter = 1u32;
    let mut offset = 0usize;
    while offset < data.len() {
        let keystream = chacha20_block(&subkey, &chacha_nonce, counter);
        let chunk_len = (data.len() - offset).min(CHACHA20_BLOCK_SIZE);
        for idx in 0..chunk_len {
            data[offset + idx] ^= keystream[idx];
        }
        counter = counter.wrapping_add(1);
        offset += chunk_len;
    }
}

fn hchacha20(key: &[u8; KEY_LEN], nonce: &[u8; 16]) -> [u8; KEY_LEN] {
    let mut state = [0u32; 16];
    state[0] = 0x6170_7865;
    state[1] = 0x3320_646e;
    state[2] = 0x7962_2d32;
    state[3] = 0x6b20_6574;
    for idx in 0..8 {
        state[4 + idx] = u32::from_le_bytes(key[idx * 4..idx * 4 + 4].try_into().unwrap());
    }
    for idx in 0..4 {
        state[12 + idx] =
            u32::from_le_bytes(nonce[idx * 4..idx * 4 + 4].try_into().unwrap());
    }

    let mut work = state;
    for _ in 0..10 {
        quarter_round(&mut work, 0, 4, 8, 12);
        quarter_round(&mut work, 1, 5, 9, 13);
        quarter_round(&mut work, 2, 6, 10, 14);
        quarter_round(&mut work, 3, 7, 11, 15);
        quarter_round(&mut work, 0, 5, 10, 15);
        quarter_round(&mut work, 1, 6, 11, 12);
        quarter_round(&mut work, 2, 7, 8, 13);
        quarter_round(&mut work, 3, 4, 9, 14);
    }

    let words = [
        work[0], work[1], work[2], work[3],
        work[12], work[13], work[14], work[15],
    ];
    let mut out = [0u8; KEY_LEN];
    for (idx, word) in words.iter().enumerate() {
        out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

fn chacha20_block(key: &[u8; KEY_LEN], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0] = 0x6170_7865;
    state[1] = 0x3320_646e;
    state[2] = 0x7962_2d32;
    state[3] = 0x6b20_6574;
    for idx in 0..8 {
        state[4 + idx] = u32::from_le_bytes(key[idx * 4..idx * 4 + 4].try_into().unwrap());
    }
    state[12] = counter;
    for idx in 0..3 {
        state[13 + idx] =
            u32::from_le_bytes(nonce[idx * 4..idx * 4 + 4].try_into().unwrap());
    }

    let mut work = state;
    for _ in 0..10 {
        quarter_round(&mut work, 0, 4, 8, 12);
        quarter_round(&mut work, 1, 5, 9, 13);
        quarter_round(&mut work, 2, 6, 10, 14);
        quarter_round(&mut work, 3, 7, 11, 15);
        quarter_round(&mut work, 0, 5, 10, 15);
        quarter_round(&mut work, 1, 6, 11, 12);
        quarter_round(&mut work, 2, 7, 8, 13);
        quarter_round(&mut work, 3, 4, 9, 14);
    }

    for (idx, word) in work.iter_mut().enumerate() {
        *word = word.wrapping_add(state[idx]);
    }

    let mut out = [0u8; 64];
    for (idx, word) in work.iter().enumerate() {
        out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

#[inline(always)]
fn array_ref_16(input: &[u8]) -> &[u8; 16] {
    input.try_into().expect("XChaCha20 nonce prefix must be 16 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_small_buffer() {
        let key = [0x42u8; KEY_LEN];
        let nonce = [0x24u8; XCHACHA20_NONCE_LEN];
        let mut data = *b"exoos-security";
        let original = data;
        let mut tag = [0u8; TAG_LEN];

        xchacha20_poly1305_seal(&key, &nonce, &mut data, b"aad", &mut tag).unwrap();
        assert_ne!(data, original);

        xchacha20_poly1305_open(&key, &nonce, &mut data, b"aad", &tag).unwrap();
        assert_eq!(data, original);
    }

    #[test]
    fn tampered_tag_is_rejected() {
        let key = [0x11u8; KEY_LEN];
        let nonce = [0x22u8; XCHACHA20_NONCE_LEN];
        let mut data = *b"tamper-test-data";
        let mut tag = [0u8; TAG_LEN];

        xchacha20_poly1305_seal(&key, &nonce, &mut data, b"", &mut tag).unwrap();
        tag[0] ^= 0x80;

        assert_eq!(
            xchacha20_poly1305_open(&key, &nonce, &mut data, b"", &tag),
            Err(AeadError::AuthenticationFailed)
        );
    }
}
