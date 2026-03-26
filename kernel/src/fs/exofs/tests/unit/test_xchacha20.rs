//! Tests unitaires — XChaCha20-BLAKE3 AEAD (spec crypto/).
#[cfg(test)]
mod tests {
    use crate::fs::exofs::crypto::xchacha20::{XChaCha20Key, XChaCha20Poly1305, Nonce};

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key   = XChaCha20Key([0x42u8; 32]);
        let nonce = Nonce([0x11u8; 24]);
        let msg   = b"ExoFS test message";
        let (ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"aad", msg).unwrap();
        let pt = XChaCha20Poly1305::decrypt(&key, &nonce, b"aad", &ct, &tag).unwrap();
        assert_eq!(pt.as_slice(), msg);
    }

    #[test]
    fn test_tampered_rejected() {
        let key   = XChaCha20Key([0x42u8; 32]);
        let nonce = Nonce([0x11u8; 24]);
        let (mut ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"", b"data").unwrap();
        ct[0] ^= 0xFF;
        assert!(XChaCha20Poly1305::decrypt(&key, &nonce, b"", &ct, &tag).is_err());
    }
}
