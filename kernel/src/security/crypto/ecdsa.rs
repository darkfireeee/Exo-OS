//! ECDSA Operations (stubs)

use alloc::vec::Vec;

pub struct EcdsaKey {
    pub curve: &'static str,
    pub private_key: Vec<u8>,
}

pub fn ecdsa_sign(key: &EcdsaKey, data: &[u8]) -> Result<Vec<u8>, &'static str> {
    Err("ECDSA not implemented")
}

pub fn ecdsa_verify(
    public_key: &[u8],
    data: &[u8],
    signature: &[u8],
) -> Result<bool, &'static str> {
    Err("ECDSA not implemented")
}
