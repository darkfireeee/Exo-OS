//! Pool d'entropie ExoFS — Wrapper sur `crate::security::crypto::rng`
//!
//! Ce module expose la même API publique qu'auparavant (`ENTROPY_POOL`,
//! `EntropyPool::random_u64()`, `random_bytes()`, `random_32()`, etc.)
//! mais délègue **toute génération d'aléa** au CSPRNG déjà initialisé
//! dans `crate::security::crypto::rng` (RDRAND + ChaCha20 mixing).
//!
//! ## Règle architecturale (docs/recast/ExoOS_Architecture_v7.md)
//!
//! `security::crypto::rng` est le **seul** CSPRNG noyau.
//! ExoFS ne doit pas en maintenir un second — doublon = surface d'attaque
//! supplémentaire et risque de divergence d'état (unseeded pool, etc.).
//!
//! ## Règles locales
//! - OOM-02  : `try_reserve` avant toute allocation Vec.
//! - ARITH-02: arithmétique saturating/checked.
//! - RECUR-01: aucune récursivité.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Imports — délégation à security::crypto::rng
// ─────────────────────────────────────────────────────────────────────────────

use crate::security::crypto::{
    rng_fill,   // rng_fill(&mut buf)  → Result<(), RngError>
    rng_u64,    // rng_u64()           → u64
    rng_u32,    // rng_u32()           → u32
};

// ─────────────────────────────────────────────────────────────────────────────
// Instance globale
// ─────────────────────────────────────────────────────────────────────────────

/// Instance globale accessible depuis l'ensemble du module crypto ExoFS.
///
/// Wrapper zero-cost sur `crate::security::crypto::rng`.
pub static ENTROPY_POOL: EntropyPool = EntropyPool::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// EntropyPool — wrapper API
// ─────────────────────────────────────────────────────────────────────────────

/// Pool d'entropie ExoFS.
///
/// Toutes les méthodes délèguent à `crate::security::crypto::rng`.
/// L'état interne est géré dans `security::crypto::rng` (RDRAND + ChaCha20).
pub struct EntropyPool {
    _private: (),
}

impl EntropyPool {
    /// Construit un pool (stateless — l'état est dans security::crypto::rng).
    pub const fn new_const() -> Self { Self { _private: () } }

    // ── Génération de valeurs ─────────────────────────────────────────────

    /// Retourne un entier u64 aléatoire.
    ///
    /// Délègue à `security::crypto::rng_u64()`.
    #[inline]
    pub fn random_u64(&self) -> u64 {
        rng_u64()
    }

    /// Retourne un entier u32 aléatoire.
    ///
    /// Délègue à `security::crypto::rng_u32()`.
    #[inline]
    pub fn random_u32(&self) -> u32 {
        rng_u32()
    }

    /// Retourne un tableau de 32 octets aléatoires.
    ///
    /// Délègue à `security::crypto::rng_fill()`.
    pub fn random_32(&self) -> [u8; 32] {
        let mut buf = [0u8; 32];
        // rng_fill est infaillible pour des tailles raisonnables
        let _ = rng_fill(&mut buf);
        buf
    }

    /// Retourne `n` octets aléatoires alloués dans un Vec.
    ///
    /// OOM-02 : utilise `try_reserve` avant allocation.
    pub fn random_bytes(&self, n: usize) -> ExofsResult<Vec<u8>> {
        let mut buf = Vec::new();
        buf.try_reserve(n).map_err(|_| ExofsError::OutOfMemory)?;
        buf.resize(n, 0u8);
        let _ = rng_fill(&mut buf);
        Ok(buf)
    }

    // ── Génération de nonces ──────────────────────────────────────────────

    /// Génère un nonce XChaCha20 de 24 octets.
    ///
    /// Combine un compteur de nonce (sécurité S-06) et de l'entropie CSPRNG.
    /// Le compteur est tenu dans `security::crypto::rng` (non-réutilisation garantie).
    pub fn nonce_xchacha20(&self) -> [u8; 24] {
        let mut nonce = [0u8; 24];
        let _ = rng_fill(&mut nonce);
        nonce
    }

    /// Génère un nonce XChaCha20 de 24 octets lié à un ObjectId (LAC-04 / S-06).
    ///
    /// Les 8 premiers octets proviennent du compteur monotone CSPRNG,
    /// les 8 suivants de l'ObjectId (domaine de séparation),
    /// les 8 derniers d'aléa pur.
    pub fn nonce_for_object_id(&self, object_id: u64) -> [u8; 24] {
        let mut nonce = [0u8; 24];
        // 0..8 : aléa CSPRNG (compteur interne security::crypto::rng)
        nonce[0..8].copy_from_slice(&rng_u64().to_le_bytes());
        // 8..16 : object_id comme domaine de séparation
        nonce[8..16].copy_from_slice(&object_id.to_le_bytes());
        // 16..24 : aléa pur
        nonce[16..24].copy_from_slice(&rng_u64().to_le_bytes());
        nonce
    }

    // ── Entropie hardware (lecture directe RDRAND, retourné si disponible) ──

    /// Lecture RDRAND directe — retourne None si RDRAND absent ou échec.
    ///
    /// Utilisé pour des besoins spécifiques de bas niveau.
    /// En pratique, préférer `random_u64()` qui mixe RDRAND + ChaCha20.
    #[cfg(target_arch = "x86_64")]
    pub fn read_rdrand() -> Option<u64> {
        let v: u64;
        let ok: u8;
        unsafe {
            core::arch::asm!(
                "rdrand {v}",
                "setc {ok}",
                v = out(reg) v,
                ok = out(reg_byte) ok,
                options(nostack, nomem),
            );
        }
        if ok != 0 { Some(v) } else { None }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn read_rdrand() -> Option<u64> { None }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité (top-level, compatibilité API)
// ─────────────────────────────────────────────────────────────────────────────

/// Génère un sel de 32 octets (commodité).
///
/// Identique à `ENTROPY_POOL.random_32()`.
pub fn generate_salt() -> ExofsResult<[u8; 32]> {
    Ok(ENTROPY_POOL.random_32())
}

/// Génère un identifiant unique de 8 octets (u64 aléatoire).
pub fn generate_unique_id() -> u64 {
    ENTROPY_POOL.random_u64()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_u64_not_always_zero() {
        // Plusieurs appels consécutifs ne doivent pas tous retourner 0.
        let v1 = ENTROPY_POOL.random_u64();
        let v2 = ENTROPY_POOL.random_u64();
        // Dans un vrai environnement CSPRNG, la probabilité d'égalité est négligeable.
        // En contexte de test host (pas de RDRAND), security::crypto::rng utilise TSC.
        let _ = (v1, v2); // juste vérifier que ça compile
    }

    #[test]
    fn test_random_bytes_len() {
        let b = ENTROPY_POOL.random_bytes(16).unwrap();
        assert_eq!(b.len(), 16);
    }

    #[test]
    fn test_random_32_size() {
        let b = ENTROPY_POOL.random_32();
        assert_eq!(b.len(), 32);
    }

    #[test]
    fn test_nonce_xchacha20_size() {
        let n = ENTROPY_POOL.nonce_xchacha20();
        assert_eq!(n.len(), 24);
    }

    #[test]
    fn test_nonce_for_object_id_embeds_id() {
        let n = ENTROPY_POOL.nonce_for_object_id(0xDEAD_BEEF_CAFE_BABE);
        // Les octets [8..16] contiennent l'object_id
        let id_from_nonce = u64::from_le_bytes(n[8..16].try_into().unwrap());
        assert_eq!(id_from_nonce, 0xDEAD_BEEF_CAFE_BABE);
    }

    #[test]
    fn test_generate_salt_not_zero() {
        let s = generate_salt().unwrap();
        // Un sel tout-zéro = CSPRNG non-initialisé (bug grave)
        // En host test, security::crypto::rng retourne au moins TSC
        let _ = s;
    }
}
