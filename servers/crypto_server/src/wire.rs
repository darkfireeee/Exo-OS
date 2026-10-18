//! wire.rs — Format des requêtes/réponses IPC et utilitaires de sérialisation

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use exo_syscall_abi as syscall;
use crate::keystore::KEY_SIZE;

pub const CRYPTO_SERVER_ENDPOINT: u64 = 4;
pub const CRYPTO_SERVER_PID: u32 = 5;
pub const CRYPTO_PROTOCOL_VERSION: u8 = 3;
pub const CRYPTO_REQUEST_PAYLOAD_SIZE: usize = 200;
pub const CRYPTO_REPLY_DATA_SIZE: usize = 224;

pub const CRYPTO_DERIVE_KEY: u32 = 0;
pub const CRYPTO_RANDOM: u32 = 1;
pub const CRYPTO_ENCRYPT: u32 = 2;
pub const CRYPTO_DECRYPT: u32 = 3;
pub const CRYPTO_HASH: u32 = 4;
pub const CRYPTO_SIGN: u32 = 5;
pub const CRYPTO_VERIFY: u32 = 6;
pub const CRYPTO_TLS_INIT: u32 = 7;
pub const CRYPTO_TLS_HANDSHAKE: u32 = 8;
pub const CRYPTO_TLS_CLOSE: u32 = 9;
pub const CRYPTO_KEY_REVOKE: u32 = 10;
pub const CRYPTO_KEY_ROTATE: u32 = 11;
pub const CRYPTO_KEY_REVOKE_OWNER: u32 = 12;
pub const CRYPTO_KEY_STATS: u32 = 13;
pub const CRYPTO_TLS_ENCRYPT: u32 = 14;
pub const CRYPTO_TLS_DECRYPT: u32 = 15;
pub const PHOENIX_WAKE_ENTROPY: u32 = 255;

pub const CRYPTO_OK: u32 = 0;
pub const CRYPTO_ERR_ARGS: u32 = 1;
pub const CRYPTO_ERR_KEY_INVALID: u32 = 2;
pub const CRYPTO_ERR_AUTH: u32 = 3;
pub const CRYPTO_ERR_CAP: u32 = 4;
pub const CRYPTO_ERR_BUSY: u32 = 5;
pub const CRYPTO_ERR_CRYPTO_FAULT: u32 = 6;

pub const IPC_FLAG_TIMEOUT: u64 = 1 << 0;
pub const IPC_RECV_TIMEOUT_MS: u64 = 50;
pub const ETIMEDOUT: i64 = -110;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CryptoRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub payload_len: u16,
    pub version: u8,
    pub flags: u8,
    pub cap_token: syscall::ExoCapTokenWire,
    pub payload: [u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
}

const _: () = assert!(core::mem::size_of::<CryptoRequest>() <= syscall::IPC_KERNEL_MAX_MSG_SIZE);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CryptoReply {
    pub sender_pid: u32,
    pub status: u32,
    pub key_handle: u32,
    pub data_len: u16,
    pub version: u8,
    pub flags: u8,
    pub data: [u8; CRYPTO_REPLY_DATA_SIZE],
}

impl CryptoReply {
    pub const fn new(status: u32) -> Self {
        Self {
            sender_pid: 0,
            status,
            key_handle: 0,
            data_len: 0,
            version: CRYPTO_PROTOCOL_VERSION,
            flags: 0,
            data: [0u8; CRYPTO_REPLY_DATA_SIZE],
        }
    }

    pub fn write_data(&mut self, data: &[u8]) {
        let copy_len = data.len().min(self.data.len());
        self.data[..copy_len].copy_from_slice(&data[..copy_len]);
        self.data_len = copy_len as u16;
    }
}

const _: () = assert!(core::mem::size_of::<CryptoReply>() <= syscall::IPC_KERNEL_MAX_MSG_SIZE);

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
