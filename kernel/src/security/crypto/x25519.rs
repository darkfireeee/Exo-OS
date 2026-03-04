// kernel/src/security/crypto/x25519.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// X25519 Diffie-Hellman — Wrapper via crate x25519-dalek v2
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CRYPTO-CRATES : implémentation via crate RustCrypto validée IETF.
// Crate : x25519-dalek v2.x, features = ["static_secrets"]
//   - Conforme RFC 7748 (X25519 Elliptic Curve Diffie-Hellman)
//   - no_std compatible
//   - Implémentation curve25519 pure Rust, pas d'opérations en virgule flottante
//
// Usages dans Exo-OS :
//   • Échange de clés de session kernel ↔ userspace (canaux IPC sécurisés)
//   • Bootstrap des clés de chiffrement de volume (combiné avec HKDF)
// ═══════════════════════════════════════════════════════════════════════════════

use x25519_dalek::{StaticSecret, PublicKey};

/// Paire de clés X25519.
///
/// `private_key` est stockée comme tableau de bytes bruts (clé scalaire 32B).
/// `public_key`  est le point Curve25519 correspondant (32B).
#[derive(Clone)]
pub struct X25519KeyPair {
    pub public_key:  [u8; 32],
    pub private_key: [u8; 32],
}

impl X25519KeyPair {
    /// Efface les clés de la mémoire.
    pub fn zeroize(&mut self) {
        self.private_key.iter_mut().for_each(|b| *b = 0);
        self.public_key.iter_mut().for_each(|b| *b = 0);
    }
}

/// Erreur X25519.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X25519Error {
    /// Clé privée invalide.
    InvalidPrivateKey,
    /// Résultat DH invalide (point à l'infini — clé publique invalide).
    InvalidDhResult,
}

impl core::fmt::Display for X25519Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            X25519Error::InvalidPrivateKey => write!(f, "X25519: invalid private key"),
            X25519Error::InvalidDhResult   => write!(f, "X25519: invalid DH result (point at infinity)"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Génère une paire de clés X25519 à partir d'un secret statique 32B.
///
/// `secret_bytes` doit être de l'entropie cryptographique (CSPRNG kernel).
/// x25519-dalek effectue le clamping bits automatiquement (RFC 7748 §5).
pub fn x25519_keypair_from_secret(secret_bytes: &[u8; 32]) -> Result<X25519KeyPair, X25519Error> {
    let secret = StaticSecret::from(*secret_bytes);
    let public  = PublicKey::from(&secret);

    Ok(X25519KeyPair {
        private_key: secret.to_bytes(),
        public_key:  *public.as_bytes(),
    })
}

/// Calcule un secret partagé Diffie-Hellman X25519.
///
/// Retourne 32 bytes de secret partagé.
/// Le secret DOIT être passé dans HKDF avant utilisation directe comme clé.
///
/// # Erreurs
/// `X25519Error::InvalidDhResult` si le résultat est le point neutre (all-zeros),
/// ce qui indique une clé publique invalide ou un twist attack.
pub fn x25519_diffie_hellman(
    our_private: &[u8; 32],
    their_public: &[u8; 32],
) -> Result<[u8; 32], X25519Error> {
    let secret  = StaticSecret::from(*our_private);
    let their_pk = PublicKey::from(*their_public);

    let dh_result = secret.diffie_hellman(&their_pk);
    let shared    = dh_result.to_bytes();

    // Vérification contre low-order points (all-zeros = point neutre)
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
        let bob_secret   = [0x22u8; 32];

        let alice = x25519_keypair_from_secret(&alice_secret).unwrap();
        let bob   = x25519_keypair_from_secret(&bob_secret).unwrap();

        let alice_shared = x25519_diffie_hellman(&alice.private_key, &bob.public_key).unwrap();
        let bob_shared   = x25519_diffie_hellman(&bob.private_key, &alice.public_key).unwrap();

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
        // Point d'ordre faible (low-order) connu pour X25519
        // (identité sur le sous-groupe d'ordre 8)
        let low_order_point = [0u8; 32];
        let our_secret = [0x42u8; 32];
        let result = x25519_diffie_hellman(&our_secret, &low_order_point);
        assert_eq!(result, Err(X25519Error::InvalidDhResult));
    }
}
