//! RSA Operations (stubs)

use alloc::vec::Vec;

pub struct RsaKey {
    pub modulus: Vec<u8>,
    pub exponent: Vec<u8>,
}

pub fn rsa_sign(key: &RsaKey, data: &[u8]) -> Result<Vec<u8>, &'static str> {
    Err("RSA not implemented")
}

pub fn rsa_verify(key: &RsaKey, data: &[u8], signature: &[u8]) -> Result<bool, &'static str> {
    Err("RSA not implemented")
}
