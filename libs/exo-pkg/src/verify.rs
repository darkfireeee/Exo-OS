//! verify.rs — Vérification de signature cryptographique Ed25519 d'un paquet Exo-OS
//!
//! Conforme à la règle SRV-04 : Les opérations cryptographiques dynamiques sont
//! déléguées au `crypto_server` via IPC synchrone (opcode `CRYPTO_VERIFY = 6`).

use crate::PkgError;
use exo_syscall_abi as syscall;

const CRYPTO_SERVER_ENDPOINT: u64 = 4;
const CRYPTO_VERIFY: u32 = 6;
const CRYPTO_OK: u32 = 0;

/// Vérifie la signature Ed25519 d'une archive de paquet auprès de crypto_server
pub fn verify_package_signature(
    data_hash: &[u8; 32],
    public_key: &[u8; 32],
    signature: &[u8; 64],
) -> Result<bool, PkgError> {
    // Construction de la requête fil vers crypto_server
    // Payload: [32B public_key][64B signature][32B data_hash] = 128B
    let mut payload = [0u8; 128];
    payload[0..32].copy_from_slice(public_key);
    payload[32..96].copy_from_slice(signature);
    payload[96..128].copy_from_slice(data_hash);

    let mut msg_buf = [0u8; syscall::IPC_ENVELOPE_SIZE];
    msg_buf[0..4].copy_from_slice(&0u32.to_ne_bytes()); // sender_pid (noyau renseigne)
    msg_buf[4..8].copy_from_slice(&CRYPTO_VERIFY.to_ne_bytes());
    msg_buf[8..8 + 128].copy_from_slice(&payload);

    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            CRYPTO_SERVER_ENDPOINT,
            msg_buf.as_ptr() as u64,
            msg_buf.len() as u64,
            0,
            0,
            0,
        )
    };

    if rc < 0 {
        return Err(PkgError::CryptoServerError(rc));
    }

    // Réception de la réponse
    let mut reply_buf = [0u8; syscall::IPC_ENVELOPE_SIZE];
    let rcv = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            0, // Endpoint privé du thread
            reply_buf.as_mut_ptr() as u64,
            reply_buf.len() as u64,
            syscall::IPC_FLAG_TIMEOUT | 500, // Timeout 500ms
        )
    };

    if rcv < 8 {
        // En cas d'indisponibilité de crypto_server, fail-closed
        return Ok(false);
    }

    let status = u32::from_ne_bytes([reply_buf[4], reply_buf[5], reply_buf[6], reply_buf[7]]);
    Ok(status == CRYPTO_OK)
}
