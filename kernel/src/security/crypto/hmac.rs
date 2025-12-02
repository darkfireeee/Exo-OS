//! HMAC (Hash-based Message Authentication Code)
//!
//! RFC 2104 compliant implementation
//! Supports SHA-256 and SHA-512

use super::hash::{sha256, sha512};

/// HMAC-SHA256
///
/// # Arguments
/// * `key` - Secret key
/// * `data` - Data to authenticate
///
/// # Returns
/// 256-bit authentication tag
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    const IPAD: u8 = 0x36;
    const OPAD: u8 = 0x5c;

    // Prepare key
    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        // Hash long keys
        let hashed = sha256(key);
        k[..32].copy_from_slice(&hashed);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    // Inner hash: H((K ⊕ ipad) || text)
    let mut inner_key = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        inner_key[i] = k[i] ^ IPAD;
    }

    let mut inner_data = alloc::vec::Vec::with_capacity(BLOCK_SIZE + data.len());
    inner_data.extend_from_slice(&inner_key);
    inner_data.extend_from_slice(data);
    let inner_hash = sha256(&inner_data);

    // Outer hash: H((K ⊕ opad) || inner_hash)
    let mut outer_key = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        outer_key[i] = k[i] ^ OPAD;
    }

    let mut outer_data = alloc::vec::Vec::with_capacity(BLOCK_SIZE + 32);
    outer_data.extend_from_slice(&outer_key);
    outer_data.extend_from_slice(&inner_hash);
    sha256(&outer_data)
}

/// HMAC-SHA512
///
/// Same as HMAC-SHA256 but with SHA-512
pub fn hmac_sha512(key: &[u8], data: &[u8]) -> [u8; 64] {
    const BLOCK_SIZE: usize = 128; // SHA-512 block size
    const IPAD: u8 = 0x36;
    const OPAD: u8 = 0x5c;

    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = sha512(key);
        k[..64].copy_from_slice(&hashed);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut inner_key = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        inner_key[i] = k[i] ^ IPAD;
    }

    let mut inner_data = alloc::vec::Vec::with_capacity(BLOCK_SIZE + data.len());
    inner_data.extend_from_slice(&inner_key);
    inner_data.extend_from_slice(data);
    let inner_hash = sha512(&inner_data);

    let mut outer_key = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        outer_key[i] = k[i] ^ OPAD;
    }

    let mut outer_data = alloc::vec::Vec::with_capacity(BLOCK_SIZE + 64);
    outer_data.extend_from_slice(&outer_key);
    outer_data.extend_from_slice(&inner_hash);
    sha512(&outer_data)
}

/// Verify HMAC (constant-time comparison)
pub fn verify_hmac_sha256(key: &[u8], data: &[u8], tag: &[u8; 32]) -> bool {
    let computed = hmac_sha256(key, data);

    // Constant-time comparison
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= computed[i] ^ tag[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_sha256_rfc4231() {
        // RFC 4231 Test Case 1
        let key = [0x0bu8; 20];
        let data = b"Hi There";
        let tag = hmac_sha256(&key, data);

        let expected = [
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
            0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
            0x2e, 0x32, 0xcf, 0xf7,
        ];

        assert_eq!(tag, expected);
    }

    #[test]
    fn test_hmac_verify() {
        let key = b"secret key";
        let data = b"data to authenticate";
        let tag = hmac_sha256(key, data);

        assert!(verify_hmac_sha256(key, data, &tag));

        // Wrong data
        assert!(!verify_hmac_sha256(key, b"wrong data", &tag));

        // Wrong key
        assert!(!verify_hmac_sha256(b"wrong key", data, &tag));
    }
}
