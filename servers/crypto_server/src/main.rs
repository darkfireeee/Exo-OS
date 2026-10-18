#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

//! # crypto_server — Service de cryptographie (SRV-04)
//!
//! Seul service Ring 1 autorisé à exposer des primitives cryptographiques.
//! Les autres serveurs délèguent ici les opérations de dérivation, chiffrement,
//! signature, vérification et rotation/révocation des clés.
//!
//! Découpé de manière modulaire :
//! - `wire.rs` : Structures de requêtes/réponses et sérialisation.
//! - `verify.rs` : Streaming et vérification Ed25519 stricte.
//! - `tls_handler.rs` : Traitement des records et sessions TLS.
//! - `dispatch.rs` : Aiguillage des opcodes et validation des capacités.
//! - `keystore.rs` : Gestion sécurisée des clés en mémoire.
//! - `pki.rs` : Arborescence de certificats et validation racine.
//! - `tls.rs` : Implémentation du protocole TLS 1.3 / session.
//! - `xchacha20.rs` : Chiffrement symétrique pur u32 (SRV-02).

extern crate blake3;

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use exo_syscall_abi as syscall;

mod dispatch;
mod keystore;
mod pki;
mod tls;
mod tls_handler;
mod verify;
mod wire;
mod xchacha20;

use dispatch::{handle_request, send_reply, IPC_RECV_TIMEOUTS};
use wire::{
    CryptoRequest, CRYPTO_PROTOCOL_VERSION, CRYPTO_REQUEST_PAYLOAD_SIZE, CRYPTO_SERVER_ENDPOINT,
    ETIMEDOUT, IPC_FLAG_TIMEOUT, IPC_RECV_TIMEOUT_MS,
};

fn boot_log(bytes: &[u8]) {
    for &byte in bytes {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = byte;
    }
}

fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

/// Entropie cryptographique RÉELLE via le CSPRNG kernel (`getrandom` → RNG durci
/// RDSEED/RDRAND + conditionnement Blake3 + ChaCha20).
pub(crate) fn secure_random(buf: &mut [u8]) -> bool {
    let r = unsafe {
        syscall::syscall3(
            syscall::SYS_GETRANDOM,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
        )
    };
    r >= 0 && (r as usize) == buf.len()
}

// FIX-SRV-M7 (ANALYSE_SERVERS §M7) : NOTE architecturale importante.
// crypto_server s'enregistre avec ENDPOINT_ID=4 (SYS_IPC_CREATE) mais son PID
// assigné par init_server est dynamique.
// L'endpoint ID (4) est la constante d'interface publique ; le PID est interne.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    boot_log(b"crypto_server: boot\n");
    xchacha20::xchacha20_init();
    keystore::keystore_init();
    tls::tls_init();

    let name = b"crypto_server";
    let register_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            CRYPTO_SERVER_ENDPOINT,
        )
    };
    if register_rc < 0 {
        boot_log(b"crypto_server: register failed\n");
        halt_forever();
    }
    boot_log(b"crypto_server: registered\n");

    let mut req = CryptoRequest {
        sender_pid: 0,
        msg_type: 0,
        reply_endpoint: 0,
        payload_len: 0,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token: syscall::ExoCapTokenWire::empty(),
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };

    loop {
        let r = unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_RECV,
                CRYPTO_SERVER_ENDPOINT,
                &mut req as *mut CryptoRequest as u64,
                core::mem::size_of::<CryptoRequest>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            let _ = keystore::expire_check();
            continue;
        }
        if r < 0 {
            continue;
        }

        let reply = handle_request(&req);
        send_reply(req.reply_endpoint, &reply);
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    boot_log(b"crypto_server: panic\n");
    halt_forever();
}
