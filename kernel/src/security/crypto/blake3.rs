// kernel/src/security/crypto/blake3.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BLAKE3 — Wrapper kernel autour de la crate blake3 (features = ["pure"])
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CRYPTO-CRATES : implémentation via crate RustCrypto validée IETF.
// JAMAIS d'implémentation from scratch (ExoOS_Dependencies_Complete.md).
//
// Crate : blake3 v1.x, features = ["pure"]
//   - "pure" : implémentation 100% Rust, aucun code assembleur natif,
//              aucune dépendance AES-NI / SIMD → compatible x86_64-unknown-none.
//   - Conforme BLAKE3 v1.3.1 (https://github.com/BLAKE3-team/BLAKE3-specs)
//
// Crate : subtle v2.x (RustCrypto) — constant-time comparisons
//   - Remplace l'implémentation maison de constant_time_eq()
//   - Conforme aux meilleures pratiques crypto (résistance timing attacks)
//
// Usages dans Exo-OS :
//   • Checksums d'intégrité kernel (superblock, blob headers)
//   • PRF dans le KDF (blake3_kdf / HKDF-BLAKE3)
//   • Vérification de signature de modules (code_signing.rs)
//   • MAC des canaux kernel (combiné avec XChaCha20)
//   • BlobId filesystem (exofs/core/blob_id.rs)
// ═══════════════════════════════════════════════════════════════════════════════

extern crate alloc;

use subtle::ConstantTimeEq;

/// Wrapper autour de `blake3::Hasher`.
///
/// L'API est volontairement conservée identique à l'ancienne interface
/// afin de ne pas casser les appelants existants.
pub struct Blake3Hasher(blake3::Hasher);

impl Blake3Hasher {
    /// Nouveau hasher standard (mode hash).
    #[inline]
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    /// Nouveau hasher avec clé 32B (mode MAC / keyed hash).
    #[inline]
    pub fn new_keyed(key: &[u8; 32]) -> Self {
        Self(blake3::Hasher::new_keyed(key))
    }

    /// Nouveau hasher en mode dérivation de clé.
    ///
    /// `context` doit être une chaîne ASCII unique et statique décrivant le
    /// contexte de domaine (ex : `b"Exo-OS 2025 Volume Encryption"`).
    /// Si `context` n'est pas de l'UTF-8 valide, le contexte est remplacé
    /// par `"ExoOS-KDF-Blake3"` (cas dégradé sécurisé).
    #[inline]
    pub fn new_derive_key(context: &[u8]) -> Self {
        let ctx = core::str::from_utf8(context).unwrap_or("ExoOS-KDF-Blake3");
        Self(blake3::Hasher::new_derive_key(ctx))
    }

    /// Ajoute des données au hasher. Retourne `&mut Self` pour le chaînage.
    #[inline]
    pub fn update(&mut self, input: &[u8]) -> &mut Self {
        self.0.update(input);
        self
    }

    /// Finalise et écrit les octets de sortie dans `out`.
    ///
    /// Si `out.len() <= 32`, sortie standard 32B.
    /// Si `out.len() > 32`, mode XOF (eXtendable Output Function).
    pub fn finalize(&self, out: &mut [u8]) {
        let n = out.len();
        if n <= 32 {
            out[..n].copy_from_slice(&self.0.finalize().as_bytes()[..n]);
        } else {
            let mut reader = self.0.finalize_xof();
            reader.fill(out);
        }
    }

    /// Hash complet d'une slice — retourne 32 bytes.
    #[inline]
    pub fn hash(input: &[u8]) -> [u8; 32] {
        blake3_hash(input)
    }

    /// MAC BLAKE3 (keyed hash) — retourne 32 bytes.
    #[inline]
    pub fn mac(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
        blake3_mac(key, input)
    }
}

impl Default for Blake3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires publiques
// ─────────────────────────────────────────────────────────────────────────────

/// Hash BLAKE3 d'un message — retourne 32 bytes.
/// Utilise `blake3::hash` (implémentation pure Rust validée IETF).
#[inline]
pub fn blake3_hash(input: &[u8]) -> [u8; 32] {
    *blake3::hash(input).as_bytes()
}

/// MAC BLAKE3 (keyed hash) d'un message avec une clé de 32 bytes.
/// Utilise `blake3::keyed_hash`.
#[inline]
pub fn blake3_mac(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    *blake3::keyed_hash(key, input).as_bytes()
}

/// Dérivation de clé BLAKE3 — contexte de domaine + matériel → clé dérivée.
///
/// `context` doit être une chaîne ASCII unique et statique.
/// `out` peut recevoir jusqu'à N octets via le mode XOF si len > 32.
/// Utilise `blake3::derive_key` (mode natif BLAKE3, plus rapide que HKDF).
pub fn blake3_derive_key(context: &[u8], material: &[u8], out: &mut [u8]) {
    let ctx = core::str::from_utf8(context).unwrap_or("ExoOS-KDF-Blake3");
    if out.len() <= 32 {
        let derived = blake3::derive_key(ctx, material);
        let n = out.len();
        out[..n].copy_from_slice(&derived[..n]);
    } else {
        let mut hasher = blake3::Hasher::new_derive_key(ctx);
        hasher.update(material);
        let mut reader = hasher.finalize_xof();
        reader.fill(out);
    }
}

/// Comparaison de deux digests en temps constant (résistance aux timing attacks).
/// Retourne `true` ssi `a == b` octet par octet.
///
/// Utilise la crate `subtle` (RustCrypto) pour garantir un temps d'exécution
/// indépendant des données comparées — remplace l'implémentation maison précédente.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = subtle::Choice::from(1);
    for (&x, &y) in a.iter().zip(b.iter()) {
        acc &= x.ct_eq(&y);
    }
    bool::from(acc)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_known_vector() {
        // Vecteur de test BLAKE3 (entrée vide)
        let h = blake3_hash(b"");
        assert_eq!(h.len(), 32);
        // BLAKE3("") = af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9...
        assert_eq!(h[0], 0xaf);
        assert_eq!(h[1], 0x13);
    }

    #[test]
    fn test_mac_different_from_hash() {
        let key = [0x42u8; 32];
        let h = blake3_hash(b"test");
        let m = blake3_mac(&key, b"test");
        assert_ne!(h, m, "MAC et hash ne doivent pas être identiques");
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"a", b"ab"));
    }

    #[test]
    fn test_hasher_update_chain() {
        let mut h1 = Blake3Hasher::new();
        h1.update(b"hel").update(b"lo");
        let mut out1 = [0u8; 32];
        h1.finalize(&mut out1);

        let out2 = blake3_hash(b"hello");
        assert_eq!(out1, out2, "update chained == single hash");
    }

    #[test]
    fn test_derive_key_domain_separation() {
        let mut k1 = [0u8; 32];
        let mut k2 = [0u8; 32];
        blake3_derive_key(b"ExoOS 2025 volume-enc", b"material", &mut k1);
        blake3_derive_key(b"ExoOS 2025 volume-mac", b"material", &mut k2);
        assert_ne!(k1, k2, "contextes différents → clés différentes");
    }
}
