// kernel/src/security/crypto/xchacha20_poly1305.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// XChaCha20-Poly1305 — STUB (non disponible sur x86_64-unknown-none)
// ═══════════════════════════════════════════════════════════════════════════════
//
// CONTRAINTE TARGET : chacha20poly1305 0.10.x dépend de poly1305 0.8.x.
// poly1305 génère des types SIMD 128 bits que LLVM tente d'abaisser via SSE2.
// Or x86_64-unknown-none désactive SSE2 dans target-features (target JSON).
// Résultat : LLVM ERROR: Do not know how to split the result of this operator!
//
// RÈGLE : En respectant l'interdiction de toute implémentation from scratch
// (ExoOS_Dependencies_Complete.md), XChaCha20-Poly1305 n'est pas disponible
// côté kernel avec ce target.
//
// ALTERNATIVE KERNEL DISPONIBLE :
//   • blake3_mac(key, data)  → MAC 256 bits sécurisé (blake3.rs)
//   • blake3_hash(data)      → hash 256 bits
//   • HKDF-SHA256            → dérivation de clé (kdf.rs)
//   • Chiffrement = flux ChaCha20 sans authentification intégrée (si nécessaire)
//     MAIS toujours coupler avec un MAC (blake3_mac) séparé.
//
// ALTERNATIVE USERSPACE : chacha20poly1305 peut être utilisé dans les serveurs
// ring 3 (servers/) où SSE2 est disponible et l'état FPU sauvegardé.
// ═══════════════════════════════════════════════════════════════════════════════

/// Longueur du tag d'authentification Poly1305 (16 octets).
pub const TAG_LEN: usize = 16;
/// Longueur du nonce XChaCha20 (24 octets).
pub const XCHACHA20_NONCE_LEN: usize = 24;
/// Longueur de la clé ChaCha20 (32 octets).
pub const KEY_LEN: usize = 32;

/// Erreur AEAD XChaCha20-Poly1305.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadError {
    /// Authentification échouée (tag invalide ou données altérées).
    AuthenticationFailed,
    /// Paramètre invalide (longueur nonce ou clé incorrecte).
    InvalidParameter,
    /// Buffer de sortie trop petit.
    BufferTooSmall,
    /// XChaCha20-Poly1305 non disponible sur ce target (poly1305 LLVM ERROR sans SSE2).
    /// Utiliser blake3_mac + HKDF pour le MAC et la dérivation de clé.
    NotAvailableOnThisTarget,
}

impl core::fmt::Display for AeadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AeadError::AuthenticationFailed     => write!(f, "XChaCha20-Poly1305: authentication failed"),
            AeadError::InvalidParameter         => write!(f, "XChaCha20-Poly1305: invalid parameter"),
            AeadError::BufferTooSmall           => write!(f, "XChaCha20-Poly1305: output buffer too small"),
            AeadError::NotAvailableOnThisTarget => write!(f, "XChaCha20-Poly1305: not available on x86_64-unknown-none (poly1305 requires SSE2); use blake3_mac"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique — stubs retournant NotAvailableOnThisTarget
// ─────────────────────────────────────────────────────────────────────────────

/// XChaCha20-Poly1305 seal — NOT AVAILABLE in kernel (see module doc).
///
/// Retourne toujours `Err(AeadError::NotAvailableOnThisTarget)`.
/// Pour l'authentification kernel, utiliser `blake3_mac` (blake3.rs).
#[inline]
pub fn xchacha20_poly1305_seal(
    _key:       &[u8; KEY_LEN],
    _nonce:     &[u8; XCHACHA20_NONCE_LEN],
    _plaintext: &mut [u8],
    _aad:       &[u8],
    _tag_out:   &mut [u8; TAG_LEN],
) -> Result<(), AeadError> {
    Err(AeadError::NotAvailableOnThisTarget)
}

/// XChaCha20-Poly1305 open — NOT AVAILABLE in kernel (see module doc).
///
/// Retourne toujours `Err(AeadError::NotAvailableOnThisTarget)`.
#[inline]
pub fn xchacha20_poly1305_open(
    _key:        &[u8; KEY_LEN],
    _nonce:      &[u8; XCHACHA20_NONCE_LEN],
    _ciphertext: &mut [u8],
    _aad:        &[u8],
    _tag:        &[u8; TAG_LEN],
) -> Result<(), AeadError> {
    Err(AeadError::NotAvailableOnThisTarget)
}
