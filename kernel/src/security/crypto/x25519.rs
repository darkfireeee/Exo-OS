// kernel/src/security/crypto/x25519.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// X25519 Diffie-Hellman — Wrapper via x25519-dalek crate v2
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE CRYPTO-CRATES: implementation via IETF-validated RustCrypto crate.
// Crate: x25519-dalek v2.x, features = ["static_secrets"]
//   - Conforms to RFC 7748 (X25519 Elliptic Curve Diffie-Hellman)
//   - no_std compatible
//   - Pure Rust curve25519 implementation, no floating point operations
//
// Usages in Exo-OS:
//   • Session key exchange kernel ↔ userspace (secure IPC channels)
//   • Volume encryption key bootstrap (combined with HKDF)
// ═══════════════════════════════════════════════════════════════════════════════

use x25519_dalek::{PublicKey, StaticSecret};

/// X25519 key pair.
///
/// `private_key` is stored as a raw byte array (32B scalar key).
/// `public_key`  is the corresponding Curve25519 point (32B).
#[derive(Clone)]
pub struct X25519KeyPair {
    pub public_key: [u8; 32],
    pub private_key: [u8; 32],
}

impl X25519KeyPair {
    /// Clears keys from memory.
    pub fn zeroize(&mut self) {
        self.private_key.iter_mut().for_each(|b| *b = 0);
        self.public_key.iter_mut().for_each(|b| *b = 0);
    }
}

/// X25519 error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X25519Error {
    /// Invalid private key.
    InvalidPrivateKey,
    /// Invalid DH result (point at infinity — invalid public key).
    InvalidDhResult,
}

impl core::fmt::Display for X25519Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            X25519Error::InvalidPrivateKey => write!(f, "X25519: invalid private key"),
            X25519Error::InvalidDhResult => {
                write!(f, "X25519: invalid DH result (point at infinity)")
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Generates an X25519 key pair from a 32B static secret.
///
/// `secret_bytes` must be cryptographic entropy (kernel CSPRNG).
/// x25519-dalek performs bit clamping automatically (RFC 7748 §5).
pub fn x25519_keypair_from_secret(secret_bytes: &[u8; 32]) -> Result<X25519KeyPair, X25519Error> {
    let secret = StaticSecret::from(*secret_bytes);
    let public = PublicKey::from(&secret);

    Ok(X25519KeyPair {
        private_key: secret.to_bytes(),
        public_key: *public.as_bytes(),
    })
}

/// Computes an X25519 Diffie-Hellman shared secret.
///
/// Returns 32 bytes of shared secret.
/// The secret MUST be passed through HKDF before direct use as a key.
///
/// # Errors
/// `X25519Error::InvalidDhResult` if the result is the neutral point (all-zeros),
/// which indicates an invalid public key or a twist attack.
pub fn x25519_diffie_hellman(
    our_private: &[u8; 32],
    their_public: &[u8; 32],
) -> Result<[u8; 32], X25519Error> {
    let secret = StaticSecret::from(*our_private);
    let their_pk = PublicKey::from(*their_public);

    let dh_result = secret.diffie_hellman(&their_pk);
    let shared = dh_result.to_bytes();

    // Check against low-order points (all-zeros = neutral point)
    let is_zero = shared.iter().fold(0u8, |acc, &b| acc | b);
    if is_zero == 0 {
        return Err(X25519Error::InvalidDhResult);
    }

    Ok(shared)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dh_symmetric() {
        let alice_secret = [0x11u8; 32];
        let bob_secret = [0x22u8; 32];

        let alice = x25519_keypair_from_secret(&alice_secret).unwrap();
        let bob = x25519_keypair_from_secret(&bob_secret).unwrap();

        let alice_shared = x25519_diffie_hellman(&alice.private_key, &bob.public_key).unwrap();
        let bob_shared = x25519_diffie_hellman(&bob.private_key, &alice.public_key).unwrap();

        assert_eq!(alice_shared, bob_shared, "DH doit être symétrique");
    }

    #[test]
    fn test_different_secrets_different_keys() {
        let kp1 = x25519_keypair_from_secret(&[0x01u8; 32]).unwrap();
        let kp2 = x25519_keypair_from_secret(&[0x02u8; 32]).unwrap();
        assert_ne!(kp1.public_key, kp2.public_key);
    }

    #[test]
    fn test_low_order_point_rejected() {
        // Known low-order point for X25519
        // (identity on subgroup of order 8)
        let low_order_point = [0u8; 32];
        let our_secret = [0x42u8; 32];
        let result = x25519_diffie_hellman(&our_secret, &low_order_point);
        assert_eq!(result, Err(X25519Error::InvalidDhResult));
    }
}
