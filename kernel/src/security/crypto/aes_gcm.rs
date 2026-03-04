// kernel/src/security/crypto/aes_gcm.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// AES-256-GCM — STUB (non disponible sur x86_64-unknown-none)
// ═══════════════════════════════════════════════════════════════════════════════
//
// CONTRAINTE TARGET : aes-gcm 0.10.x dépend de ghash → polyval.
// polyval génère des types [u64; 2] (vecteurs 128 bits) que LLVM abaisse via
// des registres XMM (SSE2). Or le target x86_64-unknown-none désactive SSE2
// dans target-features (pour ne pas corrompre l'état FPU en contexte kernel).
// Résultat : LLVM ERROR: Do not know how to split the result of this operator!
//
// RÈGLE : En respectant l'interdiction de toute implémentation from scratch
// (ExoOS_Dependencies_Complete.md), AES-GCM n'est pas disponible côté kernel.
//
// ALTERNATIVE RECOMMANDÉE : utiliser XChaCha20-Poly1305 (xchacha20_poly1305.rs)
// qui ne dépend pas de polyval et compile sans SSE2. Pour le chiffrement de
// blocs filesystem, XChaCha20-Poly1305 est aussi sécurisé qu'AES-256-GCM.
//
// NOTE : aes-gcm PEUT être utilisé dans les serveurs userspace (ring 3) où
// l'état FPU/SSE est sauvegardé par le contexte de thread.
// ═══════════════════════════════════════════════════════════════════════════════

/// Longueur de la clé AES-256 (32 octets).
pub const AES_KEY_LEN: usize = 32;
/// Longueur du nonce AES-GCM (12 octets = 96 bits, recommandé NIST).
pub const AES_GCM_NONCE_LEN: usize = 12;
/// Longueur du tag GCM (16 octets = 128 bits).
pub const AES_GCM_TAG_LEN: usize = 16;

/// Erreur AES-GCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AesGcmError {
    /// Authentification échouée (tag invalide ou données corrompues).
    AuthenticationFailed,
    /// Paramètre invalide (longueur incorrecte).
    InvalidParameter,
    /// AES-GCM non disponible sur ce target (x86_64-unknown-none sans SSE2).
    /// Utiliser XChaCha20-Poly1305 à la place (xchacha20_poly1305.rs).
    NotAvailableOnThisTarget,
}

impl core::fmt::Display for AesGcmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AesGcmError::AuthenticationFailed     => write!(f, "AES-256-GCM: authentication failed"),
            AesGcmError::InvalidParameter         => write!(f, "AES-256-GCM: invalid parameter"),
            AesGcmError::NotAvailableOnThisTarget => write!(f, "AES-256-GCM: not available on x86_64-unknown-none (no SSE2); use XChaCha20-Poly1305"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique — stubs retournant NotAvailableOnThisTarget
// ─────────────────────────────────────────────────────────────────────────────

/// AES-256-GCM seal — NOT AVAILABLE in kernel (see module doc).
///
/// Retourne toujours `Err(AesGcmError::NotAvailableOnThisTarget)`.
/// Utiliser `xchacha20_poly1305_seal` à la place.
#[inline]
pub fn aes_gcm_seal(
    _key:       &[u8; AES_KEY_LEN],
    _iv:        &[u8; AES_GCM_NONCE_LEN],
    _aad:       &[u8],
    _plaintext: &mut [u8],
    _tag_out:   &mut [u8; AES_GCM_TAG_LEN],
) -> Result<(), AesGcmError> {
    Err(AesGcmError::NotAvailableOnThisTarget)
}

/// AES-256-GCM open — NOT AVAILABLE in kernel (see module doc).
///
/// Retourne toujours `Err(AesGcmError::NotAvailableOnThisTarget)`.
/// Utiliser `xchacha20_poly1305_open` à la place.
#[inline]
pub fn aes_gcm_open(
    _key:        &[u8; AES_KEY_LEN],
    _iv:         &[u8; AES_GCM_NONCE_LEN],
    _aad:        &[u8],
    _ciphertext: &mut [u8],
    _tag:        &[u8; AES_GCM_TAG_LEN],
) -> Result<(), AesGcmError> {
    Err(AesGcmError::NotAvailableOnThisTarget)
}
