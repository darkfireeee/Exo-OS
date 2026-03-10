#![no_std]
#![no_main]

//! # crypto_server — PID 4, Service de cryptographie (SRV-04)
//!
//! Seul service autorisé à accéder aux primitives cryptographiques bas-niveau.
//! Tous les autres servers délèguent ici (SRV-02 : pas d'imports RustCrypto ailleurs).
//!
//! ## Protocole IPC (messages entrants)
//! Les clients envoient des requêtes via SYS_IPC_SEND vers l'endpoint "crypto_server".
//!
//! ### Types de requêtes (msg_type)
//! - CRYPTO_DERIVE_KEY  (0) : dérivation de clé (HKDF-Blake3)
//! - CRYPTO_RANDOM      (1) : octets aléatoires CSPRNG (via /dev/urandom kernel)
//! - CRYPTO_ENCRYPT     (2) : chiffrement XChaCha20-Poly1305
//! - CRYPTO_DECRYPT     (3) : déchiffrement XChaCha20-Poly1305
//! - CRYPTO_HASH        (4) : hash Blake3 d'un buffer
//!
//! ## Sécurité
//! - Les clés ne quittent JAMAIS ce processus (pas de réponse avec clé brute).
//! - Seul un handle opaque (key_handle u32) est retourné au client.
//! - Les handles sont invalidés à l'arrêt (pas de persistance sans ExoFS).

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

mod syscall {
    #[inline(always)]
    pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1, in("rsi") a2, in("rdx") a3,
            in("r10") a4, in("r8")  a5, in("r9")  a6,
            lateout("rax") ret,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
        syscall6(nr, a1, a2, a3, 0, 0, 0)
    }

    pub const SYS_IPC_REGISTER: u64 = 300;
    pub const SYS_IPC_RECV:     u64 = 301;
    pub const SYS_IPC_SEND:     u64 = 302;
    /// SYS_GETRANDOM = 318 (Linux-compatible, implémenté dans le kernel)
    pub const SYS_GETRANDOM:    u64 = 318;
}

// ── Types de messages crypto ──────────────────────────────────────────────────

const CRYPTO_DERIVE_KEY: u32 = 0;
const CRYPTO_RANDOM:     u32 = 1;
const CRYPTO_ENCRYPT:    u32 = 2;
const CRYPTO_DECRYPT:    u32 = 3;
const CRYPTO_HASH:       u32 = 4;

const CRYPTO_OK:         u32 = 0;
const CRYPTO_ERR_ARGS:   u32 = 1;
const CRYPTO_ERR_BUSY:   u32 = 2;

/// Message IPC entrant (128 bytes).
#[repr(C)]
struct CryptoRequest {
    sender_pid: u32,
    msg_type:   u32,
    payload:    [u8; 120],
}

/// Réponse IPC (64 bytes).
#[repr(C)]
struct CryptoReply {
    status:     u32,
    key_handle: u32,   // handle opaque (non-zero si succès DERIVE_KEY)
    data:       [u8; 56],
}

// ── Keystore en mémoire (pas de heap → table statique) ────────────────────────

/// Table des clés dérivées : max 32 handles simultanés.
/// Le handle est l'index + 1 (0 = invalide).
static KEY_TABLE_LEN: AtomicU32 = AtomicU32::new(0);
/// Chaque "clé" est représentée par 32 octets (256 bits).
static mut KEY_TABLE: [[u8; 32]; 32] = [[0u8; 32]; 32];

/// Dérivation de clé minimaliste : XOR+hash de l'input sur 32 octets.
/// En production ceci sera remplacé par HKDF-Blake3 via libs/exo_crypto.
fn derive_key_stub(material: &[u8], output: &mut [u8; 32]) {
    // FNV-128 simplifié → 32 octets de sortie
    let mut state: [u64; 4] = [
        0x6c62272e07bb0142,
        0x62b821756295c58d,
        0x0000000000000000,
        0xffffffffffffffff,
    ];
    for &b in material {
        state[0] = state[0].wrapping_mul(1099511628211).wrapping_add(b as u64);
        state[1] ^= state[0].rotate_left(17);
        state[2] = state[2].wrapping_add(state[1]);
        state[3] ^= state[2].rotate_right(11);
    }
    // Écrire les 4 × 8 = 32 octets
    output[0..8].copy_from_slice(&state[0].to_le_bytes());
    output[8..16].copy_from_slice(&state[1].to_le_bytes());
    output[16..24].copy_from_slice(&state[2].to_le_bytes());
    output[24..32].copy_from_slice(&state[3].to_le_bytes());
}

fn handle_request(req: &CryptoRequest) -> CryptoReply {
    let mut reply = CryptoReply { status: CRYPTO_ERR_ARGS, key_handle: 0, data: [0u8; 56] };

    match req.msg_type {
        CRYPTO_DERIVE_KEY => {
            let idx = KEY_TABLE_LEN.load(Ordering::Relaxed) as usize;
            if idx >= 32 {
                reply.status = CRYPTO_ERR_BUSY;
                return reply;
            }
            let material = &req.payload[..req.payload.len()];
            unsafe { derive_key_stub(material, &mut KEY_TABLE[idx]); }
            KEY_TABLE_LEN.store((idx + 1) as u32, Ordering::Release);
            reply.status = CRYPTO_OK;
            reply.key_handle = (idx + 1) as u32;
        }

        CRYPTO_RANDOM => {
            // Demande N octets aléatoires au kernel (max 56 pour tenir dans data[])
            let n = (req.payload[0] as usize).min(56);
            let r = unsafe {
                syscall::syscall3(
                    syscall::SYS_GETRANDOM,
                    reply.data.as_mut_ptr() as u64,
                    n as u64,
                    0, // flags = 0 (non-blocking best-effort)
                )
            };
            if r >= 0 { reply.status = CRYPTO_OK; }
        }

        CRYPTO_HASH => {
            // Hash Blake3 stub (FNV sur 32 octets) → data[0..32]
            let mut out = [0u8; 32];
            derive_key_stub(&req.payload, &mut out);
            reply.data[..32].copy_from_slice(&out);
            reply.status = CRYPTO_OK;
        }

        // Chiffrement/déchiffrement : délégués à des sous-modules futurs.
        // Pour Phase 5 : retourner CRYPTO_ERR_ARGS en attendant l'intégration
        // de libs/exo_crypto avec XChaCha20-Poly1305.
        CRYPTO_ENCRYPT | CRYPTO_DECRYPT => {
            reply.status = CRYPTO_ERR_ARGS;
        }

        _ => {}
    }

    reply
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 1. S'enregistrer auprès de l'ipc_router ──────────────────────────────
    let name = b"crypto_server";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            4u64, // endpoint_id = PID 4
        )
    };

    // ── 2. Boucle principale ──────────────────────────────────────────────────
    let mut req = CryptoRequest { sender_pid: 0, msg_type: 0, payload: [0u8; 120] };

    loop {
        let r = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_RECV,
                &mut req as *mut CryptoRequest as u64,
                core::mem::size_of::<CryptoRequest>() as u64,
                0,
            )
        };

        if r < 0 { continue; }

        let reply = handle_request(&req);

        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                req.sender_pid as u64,
                &reply as *const CryptoReply as u64,
                core::mem::size_of::<CryptoReply>() as u64,
                0, 0, 0,
            )
        };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}
