// kernel/src/security/crypto/ed25519.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Ed25519 — Wrapper via ed25519-dalek crate v2
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE CRYPTO-CRATES: implementation via IETF-validated RustCrypto crate.
// Crate: ed25519-dalek v2.x, features = [] (default-features = false)
//   - Conforms to RFC 8032 (Edwards-Curve Digital Signature Algorithm)
//   - SHA-512 as internal hash function (IETF standard)
//   - no_std compatible; no floating point operations
//
// NOTE: The previous implementation used BLAKE3 as internal hash
// function (non-standard). Key pairs are INCOMPATIBLE with legacy
// code. This is acceptable as no keys are deployed in production.
//
// Usages in Exo-OS:
//   • Bootloader signature verification (secure_boot.rs)
//   • Kernel module signing (code_signing.rs)
//   • TCB (Trusted Computing Base) attestation
// ═══════════════════════════════════════════════════════════════════════════════

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Ed25519 key pair.
///
/// `seed`: 32B entropy (scalar secret key before expansion)
/// `public_key`: 32B public key (point on Ed25519)
pub struct Ed25519KeyPair {
    /// 32-byte seed (RFC 8032 §5.1.5)
    pub seed: [u8; 32],
    /// 32-byte public key
    pub public_key: [u8; 32],
    // The expanded key is recomputed on demand in ed25519-dalek.
    // We keep the field for API compatibility with the old interface.
    pub expanded: [u8; 64],
}

impl Ed25519KeyPair {
    /// Clears sensitive keys from memory.
    pub fn zeroize(&mut self) {
        self.seed.iter_mut().for_each(|b| *b = 0);
        self.expanded.iter_mut().for_each(|b| *b = 0);
    }
}

/// Ed25519 error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ed25519Error {
    /// Invalid key (incorrect format).
    InvalidKey,
    /// Invalid signature (verification failed).
    InvalidSignature,
    /// Message too long or incorrect parameter.
    InvalidParameter,
}

impl core::fmt::Display for Ed25519Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Ed25519Error::InvalidKey => write!(f, "Ed25519: invalid key"),
            Ed25519Error::InvalidSignature => write!(f, "Ed25519: invalid signature"),
            Ed25519Error::InvalidParameter => write!(f, "Ed25519: invalid parameter"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Generates an Ed25519 key pair from a 32B seed.
///
/// `seed` must be cryptographic entropy (kernel CSPRNG).
/// The expanded key (`expanded` field) is computed from the seed via SHA-512
/// in accordance with RFC 8032 §5.1.5.
pub fn ed25519_keypair_from_seed(seed: &[u8; 32]) -> Result<Ed25519KeyPair, Ed25519Error> {
    let signing_key = SigningKey::from_bytes(seed);
    let public_key = signing_key.verifying_key().to_bytes();

    // Expanded key computation for API compatibility
    // Expanded key = SHA-512(seed) with bit modifications (clamping)
    // Stored here to satisfy interface but not directly used.
    let mut expanded = [0u8; 64];
    {
        use sha2::{Digest, Sha512};
        let hash = Sha512::digest(seed);
        expanded.copy_from_slice(&hash);
        // Clamping RFC 8032 §5.1.5
        expanded[0] &= 248;
        expanded[31] &= 127;
        expanded[31] |= 64;
    }

    Ok(Ed25519KeyPair {
        seed: *seed,
        public_key,
        expanded,
    })
}

/// Signs a message with the Ed25519 private key.
///
/// Returns 64-byte signature (RFC 8032 format).
pub fn ed25519_sign(keypair: &Ed25519KeyPair, message: &[u8]) -> Result<[u8; 64], Ed25519Error> {
    let signing_key = SigningKey::from_bytes(&keypair.seed);
    let sig: Signature = signing_key.sign(message);
    Ok(sig.to_bytes())
}

/// Verifies an Ed25519 signature in **strict** mode (RFC 8032 + hardenings).
///
/// Returns `Ok(())` if the signature is valid, `Err(Ed25519Error::InvalidSignature)` otherwise.
///
/// # SECURITY — `verify_strict`, not `verify`
/// We **deliberately** use `VerifyingKey::verify_strict` and not `verify`:
/// - **Rejection of weak public keys** (low-order / torsion): with a weak
///   key, an attacker can forge a valid signature for *any*
///   message. `verify` accepts them; `verify_strict` rejects them.
/// - **Anti-malleability**: `verify_strict` enforces canonical encoding of R and
///   rejects torsion components (cofactor 8 of Curve25519). Two
///   distinct signatures therefore cannot validate the same message.
/// For code/secure-boot verification (forgery = unauthorized code
/// execution), this is the only correct choice. See ed25519-dalek `verify_strict` doc.
pub fn ed25519_verify(
    public_key: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), Ed25519Error> {
    let vk = VerifyingKey::from_bytes(public_key).map_err(|_| Ed25519Error::InvalidKey)?;
    let sig = Signature::from_bytes(signature);

    vk.verify_strict(message, &sig)
        .map_err(|_| Ed25519Error::InvalidSignature)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_verify_roundtrip() {
        let seed = [0x42u8; 32];
        let kp = ed25519_keypair_from_seed(&seed).unwrap();
        let msg = b"ExoOS kernel module signature test";

        let sig = ed25519_sign(&kp, msg).unwrap();
        ed25519_verify(&kp.public_key, msg, &sig).unwrap(); // must succeed
    }

    #[test]
    fn test_tampered_message_rejected() {
        let seed = [0x11u8; 32];
        let kp = ed25519_keypair_from_seed(&seed).unwrap();
        let msg = b"original message";

        let sig = ed25519_sign(&kp, msg).unwrap();
        let tampered = b"modified message";
        let result = ed25519_verify(&kp.public_key, tampered, &sig);
        assert_eq!(result, Err(Ed25519Error::InvalidSignature));
    }

    #[test]
    fn test_wrong_key_rejected() {
        let kp1 = ed25519_keypair_from_seed(&[0x01u8; 32]).unwrap();
        let kp2 = ed25519_keypair_from_seed(&[0x02u8; 32]).unwrap();
        let msg = b"test";

        let sig = ed25519_sign(&kp1, msg).unwrap();
        let result = ed25519_verify(&kp2.public_key, msg, &sig);
        assert_eq!(result, Err(Ed25519Error::InvalidSignature));
    }

    #[test]
    fn test_different_seeds_different_keys() {
        let kp1 = ed25519_keypair_from_seed(&[0xaau8; 32]).unwrap();
        let kp2 = ed25519_keypair_from_seed(&[0xbbu8; 32]).unwrap();
        assert_ne!(kp1.public_key, kp2.public_key);
    }
}
