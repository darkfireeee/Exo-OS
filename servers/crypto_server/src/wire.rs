//! wire.rs — Format des requêtes/réponses IPC et utilitaires de sérialisation

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use exo_syscall_abi as syscall;
use crate::keystore::KEY_SIZE;

pub use syscall::{
    CryptoReply, CryptoRequest, CRYPTO_DECRYPT, CRYPTO_DERIVE_KEY, CRYPTO_ENCRYPT,
    CRYPTO_ERR_ARGS, CRYPTO_ERR_AUTH, CRYPTO_ERR_BUSY, CRYPTO_ERR_CAP, CRYPTO_ERR_CRYPTO_FAULT,
    CRYPTO_ERR_KEY_INVALID, CRYPTO_HASH, CRYPTO_KEY_REVOKE, CRYPTO_KEY_REVOKE_OWNER,
    CRYPTO_KEY_ROTATE, CRYPTO_KEY_STATS, CRYPTO_OK, CRYPTO_PROTOCOL_VERSION, CRYPTO_RANDOM,
    CRYPTO_REPLY_DATA_SIZE, CRYPTO_REQUEST_PAYLOAD_SIZE, CRYPTO_SERVER_ENDPOINT, CRYPTO_SERVER_PID,
    CRYPTO_SIGN, CRYPTO_TLS_CLOSE, CRYPTO_TLS_DECRYPT, CRYPTO_TLS_ENCRYPT, CRYPTO_TLS_HANDSHAKE,
    CRYPTO_TLS_INIT, CRYPTO_VERIFY, ETIMEDOUT, IPC_FLAG_TIMEOUT, PHOENIX_WAKE_ENTROPY,
};
pub const IPC_RECV_TIMEOUT_MS: u64 = 50;

#[inline(always)]
pub fn wipe_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    core::sync::atomic::fence(Ordering::SeqCst);
}

#[inline(always)]
pub fn read_u16_le(buf: &[u8], offset: usize) -> Option<u16> {
    if offset + 2 > buf.len() {
        return None;
    }
    Some(u16::from_le_bytes([buf[offset], buf[offset + 1]]))
}

#[inline(always)]
pub fn read_u32_le(buf: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > buf.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]))
}

#[inline(always)]
pub fn read_u64_le(buf: &[u8], offset: usize) -> Option<u64> {
    if offset + 8 > buf.len() {
        return None;
    }
    Some(u64::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ]))
}

#[inline(always)]
pub fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) -> bool {
    if offset + 4 > buf.len() {
        return false;
    }
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    true
}

pub fn derive_key_hkdf(material: &[u8], output: &mut [u8; KEY_SIZE]) {
    let salt = [0u8; 32];
    let prk = blake3::keyed_hash(&salt, material);
    let okm = blake3::derive_key("Exo-OS SRV-04 KDF v1", prk.as_bytes());
    output.copy_from_slice(&okm);
}
