//! tls_handler.rs — Traitement des messages TLS (init, handshake, encrypt, decrypt, close)

use crate::tls;
use crate::wire::{
    read_u16_le, read_u32_le, CryptoReply, CRYPTO_ERR_ARGS, CRYPTO_ERR_AUTH,
    CRYPTO_ERR_BUSY, CRYPTO_ERR_KEY_INVALID, CRYPTO_OK, CRYPTO_REPLY_DATA_SIZE,
};
use crate::xchacha20::TAG_SIZE;

pub const TLS_OP_RESPOND: u8 = 0;
pub const TLS_OP_COMPLETE_CLIENT: u8 = 1;

pub fn caller_peer_pid(caller_principal: u64) -> u32 {
    caller_principal.min(u32::MAX as u64) as u32
}

pub fn tls_init_reply(payload: &[u8], caller_principal: u64, reply: &mut CryptoReply) {
    let caller_pid = caller_peer_pid(caller_principal);
    let peer_pid = read_u32_le(payload, 0).unwrap_or(caller_pid);
    let (session_handle, client_hello) = tls::tls_handshake_initiate(caller_pid, peer_pid);
    if session_handle == 0 {
        reply.status = CRYPTO_ERR_BUSY;
        return;
    }
    reply.status = CRYPTO_OK;
    reply.key_handle = session_handle;
    reply.write_data(&client_hello);
}

pub fn tls_handshake_reply(payload: &[u8], caller_principal: u64, reply: &mut CryptoReply) {
    if payload.is_empty() {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    }

    let caller_pid = caller_peer_pid(caller_principal);

    // Compatibilité simple: un ClientHello brut peut être envoyé directement.
    if payload[0] == 1 && payload.len() >= 67 {
        let (session_handle, server_hello) =
            tls::tls_handshake_respond(&payload[..67], caller_pid, caller_pid);
        if session_handle == 0 {
            reply.status = CRYPTO_ERR_AUTH;
            return;
        }
        reply.status = CRYPTO_OK;
        reply.key_handle = session_handle;
        reply.write_data(&server_hello);
        return;
    }

    match payload[0] {
        TLS_OP_RESPOND => {
            let peer_pid = read_u32_le(payload, 1).unwrap_or(caller_pid);
            let Some(hello_len) = read_u16_le(payload, 5) else {
                reply.status = CRYPTO_ERR_ARGS;
                return;
            };
            let hello_len = hello_len as usize;
            if 7 + hello_len > payload.len() {
                reply.status = CRYPTO_ERR_ARGS;
                return;
            }
            let (session_handle, server_hello) =
                tls::tls_handshake_respond(&payload[7..7 + hello_len], caller_pid, peer_pid);
            if session_handle == 0 {
                reply.status = CRYPTO_ERR_AUTH;
                return;
            }
            reply.status = CRYPTO_OK;
            reply.key_handle = session_handle;
            reply.write_data(&server_hello);
        }
        TLS_OP_COMPLETE_CLIENT => {
            let Some(session_handle) = read_u32_le(payload, 1) else {
                reply.status = CRYPTO_ERR_ARGS;
                return;
            };
            let Some(hello_len) = read_u16_le(payload, 5) else {
                reply.status = CRYPTO_ERR_ARGS;
                return;
            };
            let hello_len = hello_len as usize;
            if 7 + hello_len > payload.len() {
                reply.status = CRYPTO_ERR_ARGS;
                return;
            }
            if tls::tls_handshake_complete_client(session_handle, caller_pid, &payload[7..7 + hello_len]) {
                reply.status = CRYPTO_OK;
                reply.key_handle = session_handle;
            } else {
                reply.status = CRYPTO_ERR_AUTH;
            }
        }
        _ => {
            reply.status = CRYPTO_ERR_ARGS;
        }
    }
}

pub fn tls_encrypt_reply(payload: &[u8], caller_principal: u64, reply: &mut CryptoReply) {
    let Some(session_handle) = read_u32_le(payload, 0) else {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    };
    let Some(data_len) = read_u16_le(payload, 4) else {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    };
    let data_len = data_len as usize;
    if 6 + data_len > payload.len() || data_len + 24 + TAG_SIZE > CRYPTO_REPLY_DATA_SIZE {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    }
    let caller_pid = caller_peer_pid(caller_principal);
    let data = &payload[6..6 + data_len];
    let mut output = [0u8; CRYPTO_REPLY_DATA_SIZE];
    if tls::tls_encrypt_record(session_handle, caller_pid, data, &mut output) {
        reply.status = CRYPTO_OK;
        reply.key_handle = session_handle;
        reply.write_data(&output[..24 + data_len + TAG_SIZE]);
    } else {
        reply.status = CRYPTO_ERR_AUTH;
    }
}

pub fn tls_decrypt_reply(payload: &[u8], caller_principal: u64, reply: &mut CryptoReply) {
    let Some(session_handle) = read_u32_le(payload, 0) else {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    };
    let Some(input_len) = read_u16_le(payload, 4) else {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    };
    let input_len = input_len as usize;
    if 6 + input_len > payload.len() || input_len < 24 + TAG_SIZE {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    }
    let caller_pid = caller_peer_pid(caller_principal);
    let input = &payload[6..6 + input_len];
    let mut plaintext = [0u8; CRYPTO_REPLY_DATA_SIZE];
    if tls::tls_decrypt_record(session_handle, caller_pid, input, &mut plaintext) {
        let pt_len = input_len - 24 - TAG_SIZE;
        reply.status = CRYPTO_OK;
        reply.key_handle = session_handle;
        reply.write_data(&plaintext[..pt_len]);
    } else {
        reply.status = CRYPTO_ERR_AUTH;
    }
}

pub fn tls_close_reply(payload: &[u8], caller_principal: u64, reply: &mut CryptoReply) {
    let Some(session_handle) = read_u32_le(payload, 0) else {
        reply.status = CRYPTO_ERR_ARGS;
        return;
    };
    let caller_pid = caller_peer_pid(caller_principal);
    if tls::tls_close(session_handle, caller_pid) {
        reply.status = CRYPTO_OK;
        reply.key_handle = session_handle;
    } else {
        reply.status = CRYPTO_ERR_KEY_INVALID;
    }
}
