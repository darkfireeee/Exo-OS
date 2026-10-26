//! dispatch.rs — Autorisation des requêtes IPC et aiguillage des opérations cryptographiques

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use ed25519_dalek::{Signer, SigningKey};
use exo_syscall_abi as syscall;

use crate::keystore::{self, KeyType, KEY_SIZE};
use crate::tls_handler::{
    tls_close_reply, tls_decrypt_reply, tls_encrypt_reply, tls_handshake_reply, tls_init_reply,
};
use crate::verify::{
    alloc_verify_context, append_verify_context, finalize_verify_context, VERIFY_OP_BEGIN,
    VERIFY_OP_FINAL, VERIFY_OP_UPDATE,
};
use crate::wire::{
    read_u16_le, read_u32_le, read_u64_le, wipe_bytes, write_u32_le, CryptoReply, CryptoRequest,
    CRYPTO_DERIVE_KEY, CRYPTO_DECRYPT, CRYPTO_ENCRYPT, CRYPTO_ERR_ARGS, CRYPTO_ERR_AUTH,
    CRYPTO_ERR_BUSY, CRYPTO_ERR_CAP, CRYPTO_ERR_KEY_INVALID, CRYPTO_HASH, CRYPTO_KEY_REVOKE,
    CRYPTO_KEY_REVOKE_OWNER, CRYPTO_KEY_ROTATE, CRYPTO_KEY_STATS, CRYPTO_OK, CRYPTO_PROTOCOL_VERSION,
    CRYPTO_RANDOM, CRYPTO_REPLY_DATA_SIZE, CRYPTO_SERVER_PID, CRYPTO_SIGN, CRYPTO_TLS_CLOSE,
    CRYPTO_TLS_DECRYPT, CRYPTO_TLS_ENCRYPT, CRYPTO_TLS_HANDSHAKE, CRYPTO_TLS_INIT, CRYPTO_VERIFY,
    PHOENIX_WAKE_ENTROPY,
};
use crate::xchacha20::{self, NONCE_SIZE, TAG_SIZE};

pub static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static REQUESTS_OK: AtomicU64 = AtomicU64::new(0);
pub static REQUESTS_ERR: AtomicU64 = AtomicU64::new(0);
pub static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

pub fn hash_blake3(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

pub fn send_reply(destination: u64, reply: &CryptoReply) {
    if destination == 0 {
        return;
    }

    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            destination,
            reply as *const CryptoReply as u64,
            core::mem::size_of::<CryptoReply>() as u64,
            0,
            0,
            0,
        )
    };
}

pub fn authorize_request(req: &CryptoRequest) -> Result<u64, u32> {
    if req.cap_token.is_empty() {
        return Err(CRYPTO_ERR_CAP);
    }

    let rc = unsafe {
        syscall::exo_cap_check(
            &req.cap_token,
            syscall::EXO_CAP_RIGHT_IPC_SEND,
            CRYPTO_SERVER_PID,
            syscall::EXO_CAP_TYPE_IPC_ENDPOINT,
        )
    };
    if rc < 0 {
        return Err(CRYPTO_ERR_CAP);
    }

    let principal = req.cap_token.object_id();
    if principal == 0 {
        return Err(CRYPTO_ERR_CAP);
    }
    Ok(principal)
}

pub fn phoenix_wake_entropy_from_request(req: &CryptoRequest, payload: &[u8]) -> Option<u64> {
    let compact_entropy = read_u64_le(&req.cap_token.bytes, 0).unwrap_or(0);
    if req.reply_endpoint == 0 && compact_entropy != 0 {
        return Some(compact_entropy);
    }
    read_u64_le(payload, 0)
}

pub fn handle_request(req: &CryptoRequest) -> CryptoReply {
    let mut reply = CryptoReply::new(CRYPTO_ERR_ARGS);
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);

    if req.version != CRYPTO_PROTOCOL_VERSION || req.payload_len as usize > req.payload.len() {
        REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
        return reply;
    }

    let payload = &req.payload[..req.payload_len as usize];
    let caller_principal = if req.msg_type == PHOENIX_WAKE_ENTROPY {
        // FIX-CS-PHOENIX-01 / T20-001 : Valider la capability kernel via exo_cap_check.
        // req.sender_pid ne suffit pas car non-authentifié sans vérification de capability.
        match authorize_request(req) {
            Ok(principal) => {
                if req.sender_pid != 0 || principal > 1 {
                    reply.status = CRYPTO_ERR_CAP;
                    REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                    return reply;
                }
                principal
            }
            Err(status) => {
                reply.status = status;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }
        }
    } else {
        match authorize_request(req) {
            Ok(principal) => principal,
            Err(status) => {
                reply.status = status;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }
        }
    };

    match req.msg_type {
        CRYPTO_DERIVE_KEY => {
            if payload.is_empty() {
                reply.status = CRYPTO_ERR_ARGS;
            } else {
                let key_type = KeyType::from_u8(payload[0]).unwrap_or(KeyType::Derived);
                let mut derived_key = [0u8; KEY_SIZE];
                crate::wire::derive_key_hkdf(&payload[1..], &mut derived_key);

                let handle = keystore::insert_key(&derived_key, key_type, caller_principal);
                wipe_bytes(&mut derived_key);

                if handle != 0 {
                    reply.status = CRYPTO_OK;
                    reply.key_handle = handle;
                } else {
                    reply.status = CRYPTO_ERR_BUSY;
                }
            }
        }
        CRYPTO_RANDOM => {
            if payload.is_empty() {
                reply.status = CRYPTO_ERR_ARGS;
            } else {
                let n = (payload[0] as usize).min(reply.data.len());
                let r = unsafe {
                    syscall::syscall3(
                        syscall::SYS_GETRANDOM,
                        reply.data.as_mut_ptr() as u64,
                        n as u64,
                        0,
                    )
                };
                if r >= 0 {
                    reply.status = CRYPTO_OK;
                    reply.data_len = n as u16;
                } else {
                    reply.status = CRYPTO_ERR_BUSY;
                }
            }
        }
        CRYPTO_ENCRYPT => {
            let Some(key_handle) = read_u32_le(payload, 0) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };
            let Some(data_len) = read_u16_le(payload, 4) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };
            let data_len = data_len as usize;

            if !keystore::is_valid_handle(key_handle) {
                reply.status = CRYPTO_ERR_KEY_INVALID;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }

            if 6 + data_len > payload.len() || data_len + NONCE_SIZE + TAG_SIZE > CRYPTO_REPLY_DATA_SIZE {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }

            let Some((mut key, _)) = keystore::get_key(key_handle, caller_principal) else {
                reply.status = CRYPTO_ERR_KEY_INVALID;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };

            let mut nonce = [0u8; NONCE_SIZE];
            let plaintext = &payload[6..6 + data_len];
            let mut sealed = [0u8; CRYPTO_REPLY_DATA_SIZE];
            let ct_tag_len = data_len + TAG_SIZE;
            let sealed_len = xchacha20::xchacha20_seal(
                &key,
                plaintext,
                &[],
                &mut nonce,
                &mut sealed[NONCE_SIZE..NONCE_SIZE + ct_tag_len],
            );
            wipe_bytes(&mut key);

            if sealed_len == ct_tag_len {
                sealed[..NONCE_SIZE].copy_from_slice(&nonce);
                let total = NONCE_SIZE + sealed_len;
                reply.status = CRYPTO_OK;
                reply.key_handle = key_handle;
                reply.write_data(&sealed[..total]);
            } else {
                reply.status = CRYPTO_ERR_AUTH;
            }
        }
        CRYPTO_DECRYPT => {
            let Some(key_handle) = read_u32_le(payload, 0) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };
            let Some(sealed_len) = read_u16_le(payload, 4) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };
            let sealed_len = sealed_len as usize;

            if !keystore::is_valid_handle(key_handle) {
                reply.status = CRYPTO_ERR_KEY_INVALID;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }

            if 6 + sealed_len > payload.len() || sealed_len < NONCE_SIZE + TAG_SIZE {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }

            let Some((mut key, _)) = keystore::get_key(key_handle, caller_principal) else {
                reply.status = CRYPTO_ERR_KEY_INVALID;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };

            let nonce: &[u8; NONCE_SIZE] = match payload[6..6 + NONCE_SIZE].try_into() {
                Ok(n) => n,
                Err(_) => {
                    wipe_bytes(&mut key);
                    reply.status = CRYPTO_ERR_ARGS;
                    REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                    return reply;
                }
            };

            let ciphertext = &payload[6 + NONCE_SIZE..6 + sealed_len];
            let mut plaintext = [0u8; CRYPTO_REPLY_DATA_SIZE];
            let opened = xchacha20::xchacha20_open(&key, nonce, ciphertext, &[], &mut plaintext);
            wipe_bytes(&mut key);

            if opened != 0 {
                reply.status = CRYPTO_OK;
                reply.key_handle = key_handle;
                reply.write_data(&plaintext[..opened]);
            } else {
                reply.status = CRYPTO_ERR_AUTH;
            }
        }
        CRYPTO_HASH => {
            let hash = hash_blake3(payload);
            reply.status = CRYPTO_OK;
            reply.write_data(&hash);
        }
        CRYPTO_SIGN => {
            let Some(key_handle) = read_u32_le(payload, 0) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };
            let Some(msg_len) = read_u16_le(payload, 4) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };
            let msg_len = msg_len as usize;

            if !keystore::is_valid_handle(key_handle) {
                reply.status = CRYPTO_ERR_KEY_INVALID;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }

            if 6 + msg_len > payload.len() {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            }

            let Some((mut key_seed, _)) = keystore::get_key(key_handle, caller_principal) else {
                reply.status = CRYPTO_ERR_KEY_INVALID;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };

            let signing_key = SigningKey::from_bytes(&key_seed);
            let signature = signing_key.sign(&payload[6..6 + msg_len]).to_bytes();
            wipe_bytes(&mut key_seed);

            reply.status = CRYPTO_OK;
            reply.key_handle = key_handle;
            reply.write_data(&signature);
        }
        CRYPTO_VERIFY => {
            if payload.is_empty() {
                reply.status = CRYPTO_ERR_ARGS;
            } else {
                match payload[0] {
                    VERIFY_OP_BEGIN => {
                        if payload.len() < 99 {
                            reply.status = CRYPTO_ERR_ARGS;
                        } else {
                            let mut public_key = [0u8; 32];
                            public_key.copy_from_slice(&payload[1..33]);
                            let mut signature = [0u8; 64];
                            signature.copy_from_slice(&payload[33..97]);
                            let Some(total_len) = read_u16_le(payload, 97) else {
                                reply.status = CRYPTO_ERR_ARGS;
                                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                                return reply;
                            };

                            let ctx = alloc_verify_context(
                                caller_principal,
                                public_key,
                                signature,
                                total_len as usize,
                            );
                            if ctx == 0 {
                                reply.status = CRYPTO_ERR_BUSY;
                            } else {
                                reply.status = CRYPTO_OK;
                                reply.key_handle = ctx;
                            }
                        }
                    }
                    VERIFY_OP_UPDATE => {
                        let Some(ctx_handle) = read_u32_le(payload, 1) else {
                            reply.status = CRYPTO_ERR_ARGS;
                            REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                            return reply;
                        };
                        let Some(chunk_len) = read_u16_le(payload, 5) else {
                            reply.status = CRYPTO_ERR_ARGS;
                            REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                            return reply;
                        };
                        let chunk_len = chunk_len as usize;
                        if 7 + chunk_len > payload.len() {
                            reply.status = CRYPTO_ERR_ARGS;
                        } else if append_verify_context(
                            ctx_handle,
                            caller_principal,
                            &payload[7..7 + chunk_len],
                        ) {
                            reply.status = CRYPTO_OK;
                            reply.key_handle = ctx_handle;
                        } else {
                            reply.status = CRYPTO_ERR_ARGS;
                        }
                    }
                    VERIFY_OP_FINAL => {
                        let Some(ctx_handle) = read_u32_le(payload, 1) else {
                            reply.status = CRYPTO_ERR_ARGS;
                            REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                            return reply;
                        };

                        match finalize_verify_context(ctx_handle, caller_principal) {
                            Some(true) => {
                                reply.status = CRYPTO_OK;
                                reply.key_handle = ctx_handle;
                            }
                            Some(false) => {
                                reply.status = CRYPTO_ERR_AUTH;
                            }
                            None => {
                                reply.status = CRYPTO_ERR_ARGS;
                            }
                        }
                    }
                    _ => {
                        reply.status = CRYPTO_ERR_ARGS;
                    }
                }
            }
        }
        CRYPTO_TLS_INIT => {
            tls_init_reply(payload, caller_principal, &mut reply);
        }
        CRYPTO_TLS_HANDSHAKE => {
            tls_handshake_reply(payload, caller_principal, &mut reply);
        }
        CRYPTO_TLS_CLOSE => {
            tls_close_reply(payload, caller_principal, &mut reply);
        }
        CRYPTO_TLS_ENCRYPT => {
            tls_encrypt_reply(payload, caller_principal, &mut reply);
        }
        CRYPTO_TLS_DECRYPT => {
            tls_decrypt_reply(payload, caller_principal, &mut reply);
        }
        CRYPTO_KEY_REVOKE => {
            let Some(key_handle) = read_u32_le(payload, 0) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };

            if !keystore::is_valid_handle(key_handle)
                || keystore::get_key(key_handle, caller_principal).is_none()
            {
                reply.status = CRYPTO_ERR_KEY_INVALID;
            } else if keystore::revoke_key(key_handle) {
                reply.status = CRYPTO_OK;
                reply.key_handle = key_handle;
            } else {
                reply.status = CRYPTO_ERR_KEY_INVALID;
            }
        }
        CRYPTO_KEY_ROTATE => {
            let Some(key_handle) = read_u32_le(payload, 0) else {
                reply.status = CRYPTO_ERR_ARGS;
                REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
                return reply;
            };

            let rotated = if keystore::is_valid_handle(key_handle) {
                keystore::rotate_key(key_handle, caller_principal)
            } else {
                0
            };
            if rotated != 0 {
                reply.status = CRYPTO_OK;
                reply.key_handle = rotated;
            } else {
                reply.status = CRYPTO_ERR_KEY_INVALID;
            }
        }
        CRYPTO_KEY_REVOKE_OWNER => {
            let revoked = keystore::revoke_all_for_owner(caller_principal);
            reply.status = CRYPTO_OK;
            reply.key_handle = revoked;
        }
        CRYPTO_KEY_STATS => {
            let stats = keystore::get_stats();
            let active_counter = keystore::active_key_count();
            let mut encoded = [0u8; 20];
            let _ = write_u32_le(&mut encoded, 0, stats.active);
            let _ = write_u32_le(&mut encoded, 4, stats.expired);
            let _ = write_u32_le(&mut encoded, 8, stats.revoked);
            let _ = write_u32_le(&mut encoded, 12, stats.free);
            let _ = write_u32_le(&mut encoded, 16, active_counter);
            reply.status = CRYPTO_OK;
            reply.write_data(&encoded);
        }
        PHOENIX_WAKE_ENTROPY => {
            // FIX-CS-PHOENIX-01 : Seul le noyau (PID 0) authentifié peut déclencher
            // le réveil Phoenix et la réinitialisation des clés.
            let authenticated_kernel_wake = req.sender_pid == 0 && caller_principal == 0;
            if !authenticated_kernel_wake {
                reply.status = CRYPTO_ERR_CAP;
            } else if let Some(entropy) = phoenix_wake_entropy_from_request(req, payload) {
                xchacha20::xchacha20_reseed(entropy);
                let _ = keystore::revoke_all_pre_phoenix();
                reply.status = CRYPTO_OK;
            } else {
                reply.status = CRYPTO_ERR_ARGS;
            }
        }
        _ => {
            reply.status = CRYPTO_ERR_ARGS;
        }
    }

    if reply.status == CRYPTO_OK {
        REQUESTS_OK.fetch_add(1, Ordering::Relaxed);
    } else {
        REQUESTS_ERR.fetch_add(1, Ordering::Relaxed);
    }

    reply
}
