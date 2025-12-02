//! HSM Cryptographic Primitives
//!
//! Hardware-accelerated crypto operations

use alloc::vec::Vec;

/// Hash data using HSM
pub fn hash(data: &[u8], algorithm: HashAlgorithm) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: use HSM hardware acceleration
    // For now: fallback to software
    use crate::security::crypto::hash::sha256;

    match algorithm {
        HashAlgorithm::Sha256 => Ok(sha256(data).to_vec()),
        _ => Err("Algorithm not supported"),
    }
}

/// HMAC using HSM
pub fn hmac(key: &[u8], data: &[u8], algorithm: HashAlgorithm) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: use HSM for HMAC
    use crate::security::crypto::hmac::hmac_sha256;

    match algorithm {
        HashAlgorithm::Sha256 => Ok(hmac_sha256(key, data).to_vec()),
        _ => Err("Algorithm not supported"),
    }
}

/// Generate random bytes using HSM RNG
pub fn random(length: usize) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: use HSM hardware RNG
    // For now: fallback to software
    use crate::security::crypto::random::get_random_bytes;

    Ok(get_random_bytes(length))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}
