// kernel/src/security/crypto/ed25519.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Ed25519 — Wrapper via crate ed25519-dalek v2
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CRYPTO-CRATES : implémentation via crate RustCrypto validée IETF.
// Crate : ed25519-dalek v2.x, features = [] (default-features = false)
//   - Conforme RFC 8032 (Edwards-Curve Digital Signature Algorithm)
//   - SHA-512 comme fonction de hachage interne (standard IETF)
//   - no_std compatible; pas d'opération en virgule flottante
//
// NOTE : L'ancienne implémentation utilisait BLAKE3 comme fonction de hachage
// interne (non-standard). Les paires de clés sont INCOMPATIBLES avec l'ancien
// code. Ceci est acceptable car aucune clé n'est déployée en production.
//
// Usages dans Exo-OS :
//   • Vérification de la signature du bootloader (secure_boot.rs)
//   • Signature des modules noyau (code_signing.rs)
//   • Attestation TCB (Trusted Computing Base)
// ═══════════════════════════════════════════════════════════════════════════════

use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};

/// Paire de clés Ed25519.
///
/// `seed`       : entropie de 32B (clé secrète scalaire avant expansion)
/// `public_key` : clé publique 32B (point sur Ed25519)
pub struct Ed25519KeyPair {
    /// Graine 32 octets (RFC 8032 §5.1.5)
    pub seed: [u8; 32],
    /// Clé publique 32 octets
    pub public_key: [u8; 32],
    // La clé étendue (expanded) est recalculée à la demande dans ed25519-dalek.
    // Nous gardons le champ pour compatibilité API avec l'ancienne interface.
    pub expanded: [u8; 64],
}

impl Ed25519KeyPair {
    /// Efface les clés sensibles de la mémoire.
    pub fn zeroize(&mut self) {
        self.seed.iter_mut().for_each(|b| *b = 0);
        self.expanded.iter_mut().for_each(|b| *b = 0);
    }
}

/// Erreur Ed25519.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ed25519Error {
    /// Clé invalide (format incorrect).
    InvalidKey,
    /// Signature invalide (vérification échouée).
    InvalidSignature,
    /// Message trop long ou paramètre incorrect.
    InvalidParameter,
}

impl core::fmt::Display for Ed25519Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Ed25519Error::InvalidKey       => write!(f, "Ed25519: invalid key"),
            Ed25519Error::InvalidSignature => write!(f, "Ed25519: invalid signature"),
            Ed25519Error::InvalidParameter => write!(f, "Ed25519: invalid parameter"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Génère une paire de clés Ed25519 à partir d'une graine 32B.
///
/// `seed` doit être de l'entropie cryptographique (CSPRNG kernel).
/// La clé étendue (champ `expanded`) est calculée depuis la graine via SHA-512
/// conformément au RFC 8032 §5.1.5.
pub fn ed25519_keypair_from_seed(seed: &[u8; 32]) -> Result<Ed25519KeyPair, Ed25519Error> {
    let signing_key = SigningKey::from_bytes(seed);
    let public_key  = signing_key.verifying_key().to_bytes();

    // Calcul de l'expanded key pour compatibilité API
    // La clé étendue = SHA-512(seed) avec modification des bits (clamping)
    // Stockée ici pour répondre à l'interface mais non utilisée directement.
    let mut expanded = [0u8; 64];
    {
        use sha2::{Sha512, Digest};
        let hash = Sha512::digest(seed);
        expanded.copy_from_slice(&hash);
        // Clamping RFC 8032 §5.1.5
        expanded[0]  &= 248;
        expanded[31] &= 127;
        expanded[31] |= 64;
    }

    Ok(Ed25519KeyPair {
        seed: *seed,
        public_key,
        expanded,
    })
}

/// Signe un message avec la clé privée Ed25519.
///
/// Retourne la signature 64 octets (format RFC 8032).
pub fn ed25519_sign(
    keypair: &Ed25519KeyPair,
    message: &[u8],
) -> Result<[u8; 64], Ed25519Error> {
    let signing_key = SigningKey::from_bytes(&keypair.seed);
    let sig: Signature = signing_key.sign(message);
    Ok(sig.to_bytes())
}

/// Vérifie une signature Ed25519.
///
/// Retourne `Ok(())` si la signature est valide, `Err(Ed25519Error::InvalidSignature)` sinon.
pub fn ed25519_verify(
    public_key: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), Ed25519Error> {
    let vk  = VerifyingKey::from_bytes(public_key)
        .map_err(|_| Ed25519Error::InvalidKey)?;
    let sig = Signature::from_bytes(signature);

    vk.verify(message, &sig)
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
        let kp   = ed25519_keypair_from_seed(&seed).unwrap();
        let msg  = b"ExoOS kernel module signature test";

        let sig = ed25519_sign(&kp, msg).unwrap();
        ed25519_verify(&kp.public_key, msg, &sig).unwrap(); // doit réussir
    }

    #[test]
    fn test_tampered_message_rejected() {
        let seed = [0x11u8; 32];
        let kp   = ed25519_keypair_from_seed(&seed).unwrap();
        let msg  = b"original message";

        let sig     = ed25519_sign(&kp, msg).unwrap();
        let tampered = b"modified message";
        let result   = ed25519_verify(&kp.public_key, tampered, &sig);
        assert_eq!(result, Err(Ed25519Error::InvalidSignature));
    }

    #[test]
    fn test_wrong_key_rejected() {
        let kp1 = ed25519_keypair_from_seed(&[0x01u8; 32]).unwrap();
        let kp2 = ed25519_keypair_from_seed(&[0x02u8; 32]).unwrap();
        let msg = b"test";

        let sig    = ed25519_sign(&kp1, msg).unwrap();
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
