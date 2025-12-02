//! HSM Cryptographic Operations
//!
//! Sign, verify, encrypt, decrypt using HSM-protected keys

use super::keys::KeyHandle;
use alloc::vec::Vec;

/// HSM Operation Type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsmOperation {
    Sign,
    Verify,
    Encrypt,
    Decrypt,
    Hash,
    Hmac,
}

/// Sign data using HSM key
pub fn sign_data(
    handle: KeyHandle,
    data: &[u8],
    algorithm: SignAlgorithm,
) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    if data.is_empty() {
        return Err("Invalid data");
    }

    // In production: send sign request to HSM
    // HSM computes signature using private key that never leaves device

    // Simulated signature (would be real crypto operation in HSM)
    let mut signature = Vec::new();
    signature.extend_from_slice(b"HSM_SIGNATURE_");
    signature.extend_from_slice(&data[..data.len().min(16)]);

    Ok(signature)
}

/// Verify signature using HSM key
pub fn verify_signature(
    handle: KeyHandle,
    data: &[u8],
    signature: &[u8],
    algorithm: SignAlgorithm,
) -> Result<bool, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: send verify request to HSM
    // HSM verifies using public key

    Ok(true) // Simulated verification
}

/// Encrypt data using HSM key
pub fn encrypt_data(
    handle: KeyHandle,
    data: &[u8],
    algorithm: EncryptAlgorithm,
) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: send encrypt request to HSM

    let mut ciphertext = Vec::new();
    ciphertext.extend_from_slice(b"ENC:");
    ciphertext.extend_from_slice(data);

    Ok(ciphertext)
}

/// Decrypt data using HSM key
pub fn decrypt_data(
    handle: KeyHandle,
    ciphertext: &[u8],
    algorithm: EncryptAlgorithm,
) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: send decrypt request to HSM
    // Private key never leaves HSM

    if ciphertext.starts_with(b"ENC:") {
        Ok(ciphertext[4..].to_vec())
    } else {
        Err("Invalid ciphertext")
    }
}

/// Signature Algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignAlgorithm {
    RsaPkcs1Sha256,
    RsaPkcs1Sha512,
    RsaPssSha256,
    EcdsaP256Sha256,
    EcdsaP384Sha384,
    EdDsa,
}

/// Encryption Algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptAlgorithm {
    RsaPkcs1,
    RsaOaep,
    AesGcm,
    AesCbc,
}
