// kernel/src/security/crypto/blake3.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BLAKE3 — Kernel wrapper around blake3 crate (features = ["pure"])
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE CRYPTO-CRATES: implementation via IETF-validated RustCrypto crate.
// NEVER an implementation from scratch (ExoOS_Dependencies_Complete.md).
//
// Crate: blake3 v1.x, features = ["pure"]
//   - "pure": 100% Rust implementation, no native assembly code,
//             no AES-NI / SIMD dependency → compatible with x86_64-unknown-none.
//   - Conforms to BLAKE3 v1.3.1 (https://github.com/BLAKE3-team/BLAKE3-specs)
//
// Crate: subtle v2.x (RustCrypto) — constant-time comparisons
//   - Replaces in-house constant_time_eq() implementation
//   - Conforms to crypto best practices (timing attacks resistance)
//
// Usages in Exo-OS:
//   • Kernel integrity checksums (superblock, blob headers)
//   • PRF in KDF (blake3_kdf / HKDF-BLAKE3)
//   • Module signature verification (code_signing.rs)
//   • Kernel channel MAC (combined with XChaCha20)
//   • Filesystem BlobId (exofs/core/blob_id.rs)
// ═══════════════════════════════════════════════════════════════════════════════

extern crate alloc;

use subtle::ConstantTimeEq;

/// Wrapper around `blake3::Hasher`.
///
/// The API is intentionally kept identical to the old interface
/// in order not to break existing callers.
pub struct Blake3Hasher(blake3::Hasher);

impl Blake3Hasher {
    /// New standard hasher (hash mode).
    #[inline]
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    /// New hasher with 32B key (MAC mode / keyed hash).
    #[inline]
    pub fn new_keyed(key: &[u8; 32]) -> Self {
        Self(blake3::Hasher::new_keyed(key))
    }

    /// New hasher in key derivation mode.
    ///
    /// `context` must be a unique, static ASCII string describing the
    /// domain context (e.g.: `b"Exo-OS 2025 Volume Encryption"`).
    /// If `context` is not valid UTF-8, the context is replaced
    /// with `"ExoOS-KDF-Blake3"` (safe fallback).
    #[inline]
    pub fn new_derive_key(context: &[u8]) -> Self {
        let ctx = core::str::from_utf8(context).unwrap_or("ExoOS-KDF-Blake3");
        Self(blake3::Hasher::new_derive_key(ctx))
    }

    /// Adds data to the hasher. Returns `&mut Self` for chaining.
    #[inline]
    pub fn update(&mut self, input: &[u8]) -> &mut Self {
        self.0.update(input);
        self
    }

    /// Finalizes and writes output bytes into `out`.
    ///
    /// If `out.len() <= 32`, standard 32B output.
    /// If `out.len() > 32`, XOF (eXtendable Output Function) mode.
    pub fn finalize(&self, out: &mut [u8]) {
        let n = out.len();
        if n <= 32 {
            out[..n].copy_from_slice(&self.0.finalize().as_bytes()[..n]);
        } else {
            let mut reader = self.0.finalize_xof();
            reader.fill(out);
        }
    }

    /// Full slice hash — returns 32 bytes.
    #[inline]
    pub fn hash(input: &[u8]) -> [u8; 32] {
        blake3_hash(input)
    }

    /// BLAKE3 MAC (keyed hash) — returns 32 bytes.
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
// Public utility functions
// ─────────────────────────────────────────────────────────────────────────────

/// DIAG: signals an abnormally large blake3 input (likely corrupted length)
/// before compress_subtree_wide recursion that would overflow the kernel stack.
/// BLAKE3 hash of a message — returns 32 bytes.
/// Uses `blake3::hash` (IETF validated pure Rust implementation).
#[inline]
pub fn blake3_hash(input: &[u8]) -> [u8; 32] {
    *blake3::hash(input).as_bytes()
}

/// BLAKE3 MAC (keyed hash) of a message with a 32-byte key.
/// Uses `blake3::keyed_hash`.
#[inline]
pub fn blake3_mac(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    *blake3::keyed_hash(key, input).as_bytes()
}

/// BLAKE3 key derivation — domain context + material → derived key.
///
/// `context` must be a unique, static ASCII string.
/// `out` can receive up to N bytes via XOF mode if len > 32.
/// Uses `blake3::derive_key` (native BLAKE3 mode, faster than HKDF).
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

/// Constant-time comparison of two digests (timing attack resistance).
/// Returns `true` iff `a == b` byte for byte.
///
/// Uses `subtle` crate (RustCrypto) to guarantee execution time
/// independent of compared data — replaces previous in-house implementation.
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
        // BLAKE3 test vector (empty input)
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
