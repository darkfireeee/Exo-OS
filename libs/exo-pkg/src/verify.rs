//! verify.rs — Vérification de signature cryptographique Ed25519 d'un paquet Exo-OS
//!
//! Conforme à la règle SRV-04 / FIX-16 : délégation au `crypto_server` via le vrai
//! protocole filaire `CryptoRequest` v3 (`ExoCapTokenWire` + endpoint réponse unique +
//! flux `VERIFY_OP_BEGIN` -> `VERIFY_OP_UPDATE` -> `VERIFY_OP_FINAL` en `verify_strict`).

use crate::PkgError;
use exo_abi as syscall;
use exo_wire::{
    CryptoReply, CryptoRequest, CRYPTO_ERR_AUTH, CRYPTO_OK, CRYPTO_PROTOCOL_VERSION,
    CRYPTO_REQUEST_PAYLOAD_SIZE, CRYPTO_SERVER_ENDPOINT, CRYPTO_SERVER_PID, CRYPTO_VERIFY,
};

const VERIFY_OP_BEGIN: u8 = 0;
const VERIFY_OP_UPDATE: u8 = 1;
const VERIFY_OP_FINAL: u8 = 2;
const PKG_VERIFY_REPLY_CHANNEL: u64 = 0x504B_4701;
const PKG_VERIFY_TIMEOUT_MS: u64 = 500;

fn mint_crypto_cap_token() -> Result<syscall::ExoCapTokenWire, PkgError> {
    let mut token = syscall::ExoCapTokenWire::empty();
    let rights = syscall::EXO_CAP_RIGHT_IPC_SEND | syscall::EXO_CAP_RIGHT_IPC_RECV;
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_EXO_CAP_CREATE,
            syscall::EXO_CAP_TYPE_IPC_ENDPOINT as u64,
            rights as u64,
            CRYPTO_SERVER_PID as u64,
            token.bytes.as_mut_ptr() as u64,
        )
    };
    if rc < 0 {
        return Err(PkgError::CryptoServerError(rc));
    }
    Ok(token)
}

fn send_crypto_verify_step(
    reply_endpoint: u64,
    payload: &[u8],
) -> Result<CryptoReply, PkgError> {
    if payload.len() > CRYPTO_REQUEST_PAYLOAD_SIZE {
        return Err(PkgError::CryptoServerError(syscall::EINVAL));
    }

    let cap_token = mint_crypto_cap_token()?;
    let mut req = CryptoRequest {
        sender_pid: 0,
        msg_type: CRYPTO_VERIFY,
        reply_endpoint,
        payload_len: payload.len() as u16,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token,
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };
    req.payload[..payload.len()].copy_from_slice(payload);

    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            CRYPTO_SERVER_ENDPOINT,
            (&req as *const CryptoRequest) as u64,
            core::mem::size_of::<CryptoRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if rc < 0 {
        return Err(PkgError::CryptoServerError(rc));
    }

    let mut reply = CryptoReply::new(CRYPTO_ERR_AUTH);
    let rcv = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            reply_endpoint,
            (&mut reply as *mut CryptoReply) as u64,
            core::mem::size_of::<CryptoReply>() as u64,
            syscall::IPC_FLAG_TIMEOUT | PKG_VERIFY_TIMEOUT_MS,
        )
    };
    if rcv < core::mem::size_of::<CryptoReply>() as i64 {
        return Err(PkgError::CryptoServerError(if rcv < 0 {
            rcv
        } else {
            syscall::ETIMEDOUT
        }));
    }
    if reply.version != CRYPTO_PROTOCOL_VERSION {
        return Err(PkgError::CryptoServerError(syscall::EPROTO));
    }
    Ok(reply)
}

/// Vérifie la signature Ed25519 d'une archive de paquet auprès de `crypto_server`
/// en utilisant le protocole v3 authentifié (`BEGIN` -> `UPDATE` -> `FINAL`).
///
/// Fail-closed : retourne `Ok(false)` en cas de signature invalide (`CRYPTO_ERR_AUTH`)
/// et `Err(...)` si `crypto_server` est indisponible ou retourne une erreur protocole.
pub fn verify_package_signature(
    data_hash: &[u8; 32],
    public_key: &[u8; 32],
    signature: &[u8; 64],
) -> Result<bool, PkgError> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return Err(PkgError::CryptoServerError(syscall::EPERM));
    }
    let reply_endpoint = ((pid as u64) << 32) | PKG_VERIFY_REPLY_CHANNEL;

    // 1. VERIFY_OP_BEGIN: [0u8][public_key: 32][signature: 64][total_len: u16 LE] = 99 bytes.
    let mut begin = [0u8; 99];
    begin[0] = VERIFY_OP_BEGIN;
    begin[1..33].copy_from_slice(public_key);
    begin[33..97].copy_from_slice(signature);
    begin[97..99].copy_from_slice(&(data_hash.len() as u16).to_le_bytes());

    let begin_reply = send_crypto_verify_step(reply_endpoint, &begin)?;
    if begin_reply.status != CRYPTO_OK || begin_reply.key_handle == 0 {
        return Ok(false);
    }
    let ctx_handle = begin_reply.key_handle;

    // 2. VERIFY_OP_UPDATE: [1u8][ctx_handle: u32 LE][chunk_len: u16 LE][data_hash: 32] = 39 bytes.
    let mut update = [0u8; 39];
    update[0] = VERIFY_OP_UPDATE;
    update[1..5].copy_from_slice(&ctx_handle.to_le_bytes());
    update[5..7].copy_from_slice(&(data_hash.len() as u16).to_le_bytes());
    update[7..39].copy_from_slice(data_hash);

    let update_reply = send_crypto_verify_step(reply_endpoint, &update)?;
    if update_reply.status != CRYPTO_OK {
        return Ok(false);
    }

    // 3. VERIFY_OP_FINAL: [2u8][ctx_handle: u32 LE] = 5 bytes.
    let mut fin = [0u8; 5];
    fin[0] = VERIFY_OP_FINAL;
    fin[1..5].copy_from_slice(&ctx_handle.to_le_bytes());

    let final_reply = send_crypto_verify_step(reply_endpoint, &fin)?;
    match final_reply.status {
        CRYPTO_OK => Ok(true),
        CRYPTO_ERR_AUTH => Ok(false),
        other => Err(PkgError::CryptoServerError(-(other as i64))),
    }
}
