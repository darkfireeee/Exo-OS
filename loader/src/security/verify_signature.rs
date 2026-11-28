//! FIX-16: Cryptographic Ed25519 + BLAKE3 ELF signature verification for `exo-loader`.
//!
//! Replaces the legacy substring check (`image.windows(8).any(|w| w == b"EXOSIG\0\0")`)
//! with a structured, tamper-evident `EXOSIG` trailer verified using Ed25519 `verify_strict`:
//!
//! Layout at end of signed ELF image (108 bytes):
//!   [0..8]   magic       = b"EXOSIG\0\0"
//!   [8..12]  version     = 1u32 (little-endian)
//!   [12..44] code_blake3 = BLAKE3(image[..image.len() - 108])
//!   [44..108] signature  = Ed25519_sign(privkey, magic || version || code_blake3)
//!
//! Any raw `EXOSIG\0\0` string embedded without a valid Ed25519 signature over the
//! image BLAKE3 digest is strictly classified as `SignatureState::Invalid` (fail-closed).

use ed25519_dalek::{Signature, VerifyingKey};

pub const EXOSIG_MAGIC: [u8; 8] = *b"EXOSIG\0\0";
pub const EXOSIG_VERSION: u32 = 1;
pub const EXOSIG_SIGNED_HEADER_LEN: usize = 8 + 4 + 32;
pub const EXOSIG_RECORD_SIZE: usize = EXOSIG_SIGNED_HEADER_LEN + 64;

/// Embedded Exo-OS master Ed25519 verifying key (matches kernel ROM `MASTER_PUBLIC_KEY`).
pub const EXOSIG_MASTER_PUBLIC_KEY: [u8; 32] = [
    0x76, 0xb5, 0x51, 0x12, 0x36, 0xc3, 0x00, 0x59, 0x88, 0xf3, 0xe2, 0x37, 0xcd, 0xd0, 0xd7, 0x97,
    0x97, 0xaf, 0x31, 0x8f, 0x15, 0xb0, 0x54, 0x49, 0xf3, 0xbb, 0x6b, 0xb8, 0x6d, 0x5d, 0xd6, 0x6e,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureState {
    /// No `EXOSIG\0\0` trailer or marker present in image.
    Unsigned,
    /// Structured `EXOSIG` trailer present AND BLAKE3 + Ed25519 `verify_strict` succeeded.
    Valid,
    /// `EXOSIG` marker/trailer present, but truncated, tampered, malformed, or forged.
    Invalid,
}

/// Verifies an ELF image carrying an appended `EXOSIG` trailer against `public_key`.
pub fn verify_signature_with_key(image: &[u8], public_key: &[u8; 32]) -> SignatureState {
    if image.len() >= EXOSIG_RECORD_SIZE {
        let split = image.len() - EXOSIG_RECORD_SIZE;
        let trailer = &image[split..];
        if trailer[0..8] == EXOSIG_MAGIC {
            let version = u32::from_le_bytes([trailer[8], trailer[9], trailer[10], trailer[11]]);
            if version != EXOSIG_VERSION {
                return SignatureState::Invalid;
            }
            let expected_hash = &trailer[12..44];
            let computed = blake3::hash(&image[..split]);
            let mut diff = 0u8;
            for i in 0..32 {
                diff |= computed.as_bytes()[i] ^ expected_hash[i];
            }
            if diff != 0 {
                return SignatureState::Invalid;
            }

            let Ok(vk) = VerifyingKey::from_bytes(public_key) else {
                return SignatureState::Invalid;
            };
            let mut sig_bytes = [0u8; 64];
            sig_bytes.copy_from_slice(&trailer[44..108]);
            let sig = Signature::from_bytes(&sig_bytes);
            return if vk
                .verify_strict(&trailer[0..EXOSIG_SIGNED_HEADER_LEN], &sig)
                .is_ok()
            {
                SignatureState::Valid
            } else {
                SignatureState::Invalid
            };
        }
    }

    // Fail-closed: any forged/stray `EXOSIG\0\0` marker not forming a valid trailer is Invalid.
    if image.windows(8).any(|w| w == EXOSIG_MAGIC) {
        SignatureState::Invalid
    } else {
        SignatureState::Unsigned
    }
}

/// Verifies an ELF image against the embedded Exo-OS ROM verifying key.
pub fn detect_signature_note(image: &[u8]) -> SignatureState {
    verify_signature_with_key(image, &EXOSIG_MASTER_PUBLIC_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use ed25519_dalek::{Signer, SigningKey};

    fn sign_image(body: &[u8], seed: [u8; 32]) -> (Vec<u8>, [u8; 32]) {
        let sk = SigningKey::from_bytes(&seed);
        let pk = sk.verifying_key().to_bytes();
        let digest = blake3::hash(body);

        let mut header = [0u8; EXOSIG_SIGNED_HEADER_LEN];
        header[0..8].copy_from_slice(&EXOSIG_MAGIC);
        header[8..12].copy_from_slice(&EXOSIG_VERSION.to_le_bytes());
        header[12..44].copy_from_slice(digest.as_bytes());
        let sig = sk.sign(&header);

        let mut out = Vec::with_capacity(body.len() + EXOSIG_RECORD_SIZE);
        out.extend_from_slice(body);
        out.extend_from_slice(&header);
        out.extend_from_slice(&sig.to_bytes());
        (out, pk)
    }

    #[test]
    fn var13_unsigned_image_is_detected() {
        let elf = b"\x7fELF\x02\x01\x01\0unsigned_binary_payload";
        assert_eq!(detect_signature_note(elf), SignatureState::Unsigned);
    }

    #[test]
    fn var13_forged_exosig_string_is_rejected_as_invalid() {
        let forged = b"\x7fELF\x02\x01\x01\0attacker_payload_EXOSIG\0\0_fake";
        assert_eq!(detect_signature_note(forged), SignatureState::Invalid);
    }

    #[test]
    fn var13_valid_ed25519_signature_accepted_and_tamper_rejected() {
        let body = b"\x7fELF\x02\x01\x01\0genuine_exo_binary_code_section";
        let (mut signed, pk) = sign_image(body, [0x42u8; 32]);
        assert_eq!(verify_signature_with_key(&signed, &pk), SignatureState::Valid);

        // Wrong verifying key MUST fail.
        assert_eq!(detect_signature_note(&signed), SignatureState::Invalid);

        // Single-bit code tamper MUST invalidate the signature.
        signed[10] ^= 0x01;
        assert_eq!(verify_signature_with_key(&signed, &pk), SignatureState::Invalid);
    }
}
