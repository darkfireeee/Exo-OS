#![no_std]
//! exo-fscrypt — Crypto de chiffrement-at-rest ExoFS, **source unique de vérité**
//! partagée entre le kernel (no_std) et l'outil `exofs-mkroot` (std).
//!
//! Objectif : éliminer toute **divergence** entre le code qui *écrit* un volume
//! chiffré (mkfs) et celui qui le *lit* (kernel). Les deux appellent ces mêmes
//! fonctions → cohérence par construction.
//!
//! ## Primitives (identiques au primitif kernel `security::crypto::xchacha20_poly1305`)
//! - **XChaCha20** : RFC 8439 ChaCha20 + HChaCha20, arithmétique u32 pure (pas de
//!   crate `chacha20` : incompatible `x86_64-unknown-none`, LLVM split 128-bit).
//!   Compteur initial = 1 (compat avec le primitif kernel).
//! - **AEAD** : XChaCha20 (flux) + MAC BLAKE3 keyé (contexte
//!   `ExoOS-Kernel-XChaCha20-BLAKE3-MAC-v1`), tag tronqué 16 octets.
//! - **KEK** : Argon2id (m=65536, t=3, p=4, 32 o) — identique à
//!   `KeyDerivation::derive_from_passphrase_default`.
//! - **Clé de blob** : `blake3::derive_key("exofs-atrest-key-v1", volume_key || blob_id)`.
//! - **Nonce de bloc** : `blake3::hash(blob_id || offset_le)[..24]`.
//!
//! Un test kernel (`fs::exofs::crypto::at_rest`) vérifie l'**équivalence
//! byte-à-byte** entre l'AEAD d'ici et le primitif audité du kernel.

extern crate alloc;

use alloc::vec::Vec;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
const CHACHA20_BLOCK: usize = 64;
const MAC_CONTEXT: &str = "ExoOS-Kernel-XChaCha20-BLAKE3-MAC-v1";

// ─────────────────────────────────────────────────────────────────────────────
// XChaCha20 (u32 pur, RFC 8439) — copié du primitif kernel pour partage portable
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(7);
}

fn double_rounds(work: &mut [u32; 16]) {
    for _ in 0..10 {
        quarter_round(work, 0, 4, 8, 12);
        quarter_round(work, 1, 5, 9, 13);
        quarter_round(work, 2, 6, 10, 14);
        quarter_round(work, 3, 7, 11, 15);
        quarter_round(work, 0, 5, 10, 15);
        quarter_round(work, 1, 6, 11, 12);
        quarter_round(work, 2, 7, 8, 13);
        quarter_round(work, 3, 4, 9, 14);
    }
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
        state[13 + idx] = u32::from_le_bytes(nonce[idx * 4..idx * 4 + 4].try_into().unwrap());
    }
    let mut work = state;
    double_rounds(&mut work);
    for (idx, word) in work.iter_mut().enumerate() {
        *word = word.wrapping_add(state[idx]);
    }
    let mut out = [0u8; 64];
    for (idx, word) in work.iter().enumerate() {
        out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
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
        state[12 + idx] = u32::from_le_bytes(nonce[idx * 4..idx * 4 + 4].try_into().unwrap());
    }
    let mut work = state;
    double_rounds(&mut work);
    let words = [
        work[0], work[1], work[2], work[3], work[12], work[13], work[14], work[15],
    ];
    let mut out = [0u8; KEY_LEN];
    for (idx, word) in words.iter().enumerate() {
        out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// Chiffrement de flux XChaCha20 (XOR keystream), longueur-préservant.
/// `encrypt == decrypt`. Compteur initial = 1 (compat primitif kernel).
pub fn xchacha20_xor(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], data: &mut [u8]) {
    let subkey = hchacha20(key, nonce[..16].try_into().unwrap());
    let mut chacha_nonce = [0u8; 12];
    chacha_nonce[4..].copy_from_slice(&nonce[16..]);
    let mut counter = 1u32;
    let mut offset = 0usize;
    while offset < data.len() {
        let keystream = chacha20_block(&subkey, &chacha_nonce, counter);
        let chunk = (data.len() - offset).min(CHACHA20_BLOCK);
        for idx in 0..chunk {
            data[offset + idx] ^= keystream[idx];
        }
        counter = counter.wrapping_add(1);
        offset += chunk;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AEAD : XChaCha20 + MAC BLAKE3 keyé (identique à compute_tag du kernel)
// ─────────────────────────────────────────────────────────────────────────────

fn compute_tag(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], aad: &[u8], ct: &[u8]) -> [u8; TAG_LEN] {
    let mut ikm = [0u8; KEY_LEN + NONCE_LEN];
    ikm[..KEY_LEN].copy_from_slice(key);
    ikm[KEY_LEN..].copy_from_slice(nonce);
    let mac_key = blake3::derive_key(MAC_CONTEXT, &ikm);

    let mut hasher = blake3::Hasher::new_keyed(&mac_key);
    hasher.update(&(aad.len() as u64).to_le_bytes());
    hasher.update(aad);
    hasher.update(&(ct.len() as u64).to_le_bytes());
    hasher.update(ct);
    let full = hasher.finalize();
    let mut tag = [0u8; TAG_LEN];
    tag.copy_from_slice(&full.as_bytes()[..TAG_LEN]);
    tag
}

/// Scelle `plaintext` (chiffré en place via XOR) et retourne le tag.
pub fn aead_seal(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    buf: &mut [u8],
) -> [u8; TAG_LEN] {
    xchacha20_xor(key, nonce, buf);
    compute_tag(key, nonce, aad, buf)
}

/// Vérifie le tag (temps constant via BLAKE3) puis déchiffre en place.
/// Retourne `false` si l'authentification échoue (buffer laissé tel quel).
#[must_use]
pub fn aead_open(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    buf: &mut [u8],
    tag: &[u8; TAG_LEN],
) -> bool {
    let expected = compute_tag(key, nonce, aad, buf);
    // Comparaison constant-time.
    let mut diff = 0u8;
    for i in 0..TAG_LEN {
        diff |= expected[i] ^ tag[i];
    }
    if diff != 0 {
        return false;
    }
    xchacha20_xor(key, nonce, buf);
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// KEK (Argon2id) — paramètres identiques à derive_from_passphrase_default
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsCryptError {
    EmptyPassphrase,
    KdfFailure,
    BadFormat,
    AuthFailed,
}

/// Dérive la KEK 32 octets : Argon2id(m=65536, t=3, p=4) sur (passphrase, salt).
pub fn derive_kek(passphrase: &[u8], salt: &[u8; 32]) -> Result<[u8; 32], FsCryptError> {
    if passphrase.is_empty() {
        return Err(FsCryptError::EmptyPassphrase);
    }
    let params = argon2::Params::new(65_536, 3, 4, Some(32)).map_err(|_| FsCryptError::KdfFailure)?;
    let a2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    let mut memory: Vec<argon2::Block> = Vec::new();
    memory
        .try_reserve(a2.params().block_count())
        .map_err(|_| FsCryptError::KdfFailure)?;
    memory.resize(a2.params().block_count(), argon2::Block::default());
    a2.hash_password_into_with_memory(passphrase, salt, &mut out, memory.as_mut_slice())
        .map_err(|_| FsCryptError::KdfFailure)?;
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrap / unwrap de la clé de volume (format superblock, 110 octets)
// ─────────────────────────────────────────────────────────────────────────────

const VK_WRAP_MAGIC: [u8; 4] = *b"EXVK";
const VK_WRAP_VERSION: u8 = 1;
const VK_SOURCE_PASSPHRASE: u8 = 0;
const VK_WRAP_AAD: &[u8] = b"exofs-volume-key-wrap-v1";

/// Taille de la clé de volume wrappée : magic(4)+ver(1)+source(1)+salt(32)+
/// nonce(24)+ct(32)+tag(16) = 110.
pub const WRAPPED_VK_LEN: usize = 4 + 1 + 1 + 32 + 24 + 32 + 16;

/// Wrappe `vk` avec une KEK passphrase. `salt`/`nonce` fournis par l'appelant
/// (entropie kernel ou OsRng selon le contexte) → fonction pure et testable.
pub fn wrap_volume_key(
    vk: &[u8; 32],
    passphrase: &[u8],
    salt: &[u8; 32],
    nonce: &[u8; 24],
) -> Result<[u8; WRAPPED_VK_LEN], FsCryptError> {
    let kek = derive_kek(passphrase, salt)?;
    let mut ct = *vk;
    let tag = aead_seal(&kek, nonce, VK_WRAP_AAD, &mut ct);

    let mut out = [0u8; WRAPPED_VK_LEN];
    out[0..4].copy_from_slice(&VK_WRAP_MAGIC);
    out[4] = VK_WRAP_VERSION;
    out[5] = VK_SOURCE_PASSPHRASE;
    out[6..38].copy_from_slice(salt);
    out[38..62].copy_from_slice(nonce);
    out[62..94].copy_from_slice(&ct);
    out[94..110].copy_from_slice(&tag);
    Ok(out)
}

/// Déwrappe une clé de volume. Échoue (`AuthFailed`) si passphrase fausse ou
/// données altérées.
pub fn unwrap_volume_key(wrapped: &[u8], passphrase: &[u8]) -> Result<[u8; 32], FsCryptError> {
    if wrapped.len() < WRAPPED_VK_LEN
        || wrapped[0..4] != VK_WRAP_MAGIC
        || wrapped[4] != VK_WRAP_VERSION
    {
        return Err(FsCryptError::BadFormat);
    }
    let mut salt = [0u8; 32];
    salt.copy_from_slice(&wrapped[6..38]);
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&wrapped[38..62]);
    let mut ct = [0u8; 32];
    ct.copy_from_slice(&wrapped[62..94]);
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&wrapped[94..110]);

    let kek = derive_kek(passphrase, &salt)?;
    if !aead_open(&kek, &nonce, VK_WRAP_AAD, &mut ct, &tag) {
        return Err(FsCryptError::AuthFailed);
    }
    Ok(ct)
}

// ─────────────────────────────────────────────────────────────────────────────
// Chiffrement de blob (identique à at_rest.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Clé de chiffrement d'un blob : `blake3::derive_key("exofs-atrest-key-v1",
/// volume_key || blob_id)`.
pub fn blob_key(volume_key: &[u8; 32], blob_id: &[u8; 32]) -> [u8; 32] {
    let mut material = [0u8; 64];
    material[..32].copy_from_slice(volume_key);
    material[32..].copy_from_slice(blob_id);
    blake3::derive_key("exofs-atrest-key-v1", &material)
}

/// Nonce déterministe par (blob, offset disque).
pub fn block_nonce(blob_id: &[u8; 32], disk_offset: u64) -> [u8; 24] {
    let mut material = [0u8; 40];
    material[..32].copy_from_slice(blob_id);
    material[32..40].copy_from_slice(&disk_offset.to_le_bytes());
    let h = blake3::hash(&material);
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&h.as_bytes()[..24]);
    nonce
}

/// Chiffre/déchiffre un bloc de blob en place (involution).
pub fn xor_block(key: &[u8; 32], blob_id: &[u8; 32], disk_offset: u64, buf: &mut [u8]) {
    let nonce = block_nonce(blob_id, disk_offset);
    xchacha20_xor(key, &nonce, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vecteur RFC 8439 §2.3.2 : prouve que ChaCha20 est conforme (et donc
    /// équivalent au primitif kernel, lui aussi conforme RFC).
    #[test]
    fn chacha20_block_matches_rfc8439() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let nonce = [
            0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
        ];
        let block = chacha20_block(&key, &nonce, 1);
        // Premiers octets du keystream RFC 8439 §2.3.2.
        assert_eq!(block[0], 0x10);
        assert_eq!(block[1], 0xf1);
        assert_eq!(block[2], 0xe7);
        assert_eq!(block[3], 0xe4);
    }

    #[test]
    fn xchacha20_xor_is_involution() {
        let key = [0x42u8; 32];
        let nonce = [0x24u8; 24];
        let plain = b"exo-fscrypt at-rest payload, immutable content-addressed blob".to_vec();
        let mut buf = plain.clone();
        xchacha20_xor(&key, &nonce, &mut buf);
        assert_ne!(buf, plain);
        xchacha20_xor(&key, &nonce, &mut buf);
        assert_eq!(buf, plain);
    }

    #[test]
    fn aead_roundtrip_and_tamper() {
        let key = [0x11u8; 32];
        let nonce = [0x22u8; 24];
        let aad = b"context";
        let mut buf = [0xABu8; 48];
        let orig = buf;
        let tag = aead_seal(&key, &nonce, aad, &mut buf);
        assert_ne!(buf, orig);
        // Tamper → auth échoue.
        let mut bad = buf;
        bad[0] ^= 1;
        assert!(!aead_open(&key, &nonce, aad, &mut bad, &tag));
        // Bon → restaure.
        assert!(aead_open(&key, &nonce, aad, &mut buf, &tag));
        assert_eq!(buf, orig);
    }

    #[test]
    fn volume_key_wrap_unwrap_roundtrip() {
        let vk = [0x5Au8; 32];
        let salt = [0x01u8; 32];
        let nonce = [0x02u8; 24];
        let wrapped = wrap_volume_key(&vk, b"correct horse", &salt, &nonce).unwrap();
        assert_eq!(wrapped.len(), WRAPPED_VK_LEN);
        assert_eq!(unwrap_volume_key(&wrapped, b"correct horse").unwrap(), vk);
        assert_eq!(
            unwrap_volume_key(&wrapped, b"wrong"),
            Err(FsCryptError::AuthFailed)
        );
    }

    #[test]
    fn blob_cipher_roundtrip_and_distinct() {
        let vk = [0x7Au8; 32];
        let bid_a = [0xAAu8; 32];
        let bid_b = [0xBBu8; 32];
        let ka = blob_key(&vk, &bid_a);
        let kb = blob_key(&vk, &bid_b);
        assert_ne!(ka, kb, "clés de blobs distinctes");
        let plain = [0x33u8; 100];
        let mut a = plain;
        xor_block(&ka, &bid_a, 0, &mut a);
        assert_ne!(a, plain);
        xor_block(&ka, &bid_a, 0, &mut a);
        assert_eq!(a, plain, "roundtrip");
        // Offsets distincts → keystream distinct.
        let mut o0 = plain;
        let mut o1 = plain;
        xor_block(&ka, &bid_a, 0, &mut o0);
        xor_block(&ka, &bid_a, 512, &mut o1);
        assert_ne!(o0, o1);
    }
}
