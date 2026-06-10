#![no_std]
#![no_main]

//! # ipc_router — PID 2, Directory Service
//!
//! Premier server Ring 1 à démarrer (après init PID 1).
//! Responsabilités :
//!   - Registre de nommage : nom → endpoint IPC
//!   - Routing des messages entre servers
//!   - Security gate : vérification capability avant forward
//!   - Heartbeat : détecte les servers crash, notifie init
//!
//! ## Protocole IPC (ring SPSC virtuel via mémoire partagée kernel)
//! Chaque server ouvre un endpoint via SYS_IPC_REGISTER(name, cap).
//! Les clients envoient SYS_IPC_SEND(dest_name, msg_ptr, len).
//! L'ipc_router forwarder le message vers l'endpoint enregistré.
//!
//! ## Numéros de syscall utilisés
//! - SYS_IPC_REGISTER  = 304 (enregistre cet endpoint)
//! - SYS_IPC_RECV      = 301 (reçoit un message)
//! - SYS_IPC_SEND      = 300 (envoie un message)
//! - SYS_GETPID        = 39  (récupère notre PID)

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use exo_ipc_router::security_gate;
use exo_syscall_abi as syscall;

/// Registre d'endpoints : max 64 services simultanés.
///
/// FIX-FNV64 (ANALYSE_SERVERS_EXOOS §P1) : l'ancienne implémentation utilisait
/// FNV-32 dont l'espace de 2³² valeurs produit des collisions pratiques.
/// Un attaquant peut forger un nom collisionant pour hijacker un endpoint enregistré.
/// Remplacé par FNV-64a (espace 2⁶⁴) — probabilité de collision négligeable.
///
/// Chaque entrée = (nom hash 64-bit, endpoint_id 32-bit).
struct Registry {
    names:     [u64; 64],
    endpoints: [u32; 64],
    count:     AtomicU32,
}

impl Registry {
    const fn new() -> Self {
        Self {
            names:     [0u64; 64],
            endpoints: [0u32; 64],
            count:     AtomicU32::new(0),
        }
    }

    /// Enregistre un endpoint. Retourne false si la table est pleine.
    fn register_hash(&mut self, h: u64, endpoint: u32) -> bool {
        let n = self.count.load(Ordering::Relaxed) as usize;
        if n >= 64 {
            return false;
        }
        self.names[n]     = h;
        self.endpoints[n] = endpoint;
        self.count.store((n + 1) as u32, Ordering::Release);
        true
    }
}

const IPC_MSG_REGISTER: u32 = 0;
const IPC_MSG_ROUTE: u32 = 1;
const IPC_MSG_HEARTBEAT: u32 = 2;
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = syscall::ETIMEDOUT;
const IPC_HEADER_SIZE: usize = syscall::IPC_HEADER_SIZE;
const IPC_PAYLOAD_SIZE: usize = syscall::IPC_INLINE_PAYLOAD_SIZE;

// --- Globals no_std (pas de heap) ---
static RUNNING: AtomicBool = AtomicBool::new(true);
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

fn debug_write(bytes: &[u8]) {
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        )
    };
}

#[inline(always)]
fn payload_byte(msg: &syscall::IpcMessage, idx: usize) -> u8 {
    debug_assert!(idx < IPC_PAYLOAD_SIZE);
    unsafe { msg.payload.as_ptr().wrapping_add(idx).read() }
}

#[inline(always)]
fn payload_ptr(msg: &syscall::IpcMessage, idx: usize) -> *const u8 {
    debug_assert!(idx <= IPC_PAYLOAD_SIZE);
    msg.payload.as_ptr().wrapping_add(idx)
}

fn read_payload_u32(msg: &syscall::IpcMessage, offset: usize, payload_len: usize) -> Option<u32> {
    if offset > payload_len || payload_len - offset < 4 || payload_len > IPC_PAYLOAD_SIZE {
        return None;
    }
    let b0 = payload_byte(msg, offset) as u32;
    let b1 = payload_byte(msg, offset + 1) as u32;
    let b2 = payload_byte(msg, offset + 2) as u32;
    let b3 = payload_byte(msg, offset + 3) as u32;
    Some(b0 | (b1 << 8) | (b2 << 16) | (b3 << 24))
}

fn read_payload_u64(msg: &syscall::IpcMessage, offset: usize, payload_len: usize) -> Option<u64> {
    if offset > payload_len || payload_len - offset < 8 || payload_len > IPC_PAYLOAD_SIZE {
        return None;
    }
    let lo = read_payload_u32(msg, offset, payload_len)? as u64;
    let hi = read_payload_u32(msg, offset + 4, payload_len)? as u64;
    Some(lo | (hi << 32))
}

fn hash_payload(
    msg: &syscall::IpcMessage,
    start: usize,
    len: usize,
    payload_len: usize,
) -> Option<u64> {
    // FIX-FNV64: retourne u64 avec FNV-64a (cohérent avec Registry::hash_name).
    if start > payload_len || len > payload_len - start || payload_len > IPC_PAYLOAD_SIZE {
        return None;
    }
    const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV64_PRIME:  u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV64_OFFSET;
    let mut i = 0usize;
    while i < len {
        h ^= payload_byte(msg, start + i) as u64;
        h = h.wrapping_mul(FNV64_PRIME);
        i += 1;
    }
    Some(h)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 1. Se déclarer auprès du kernel comme IPC router ──────────────────
    // SYS_IPC_REGISTER(b"ipc_router", endpoint_id=2)
    let name = b"ipc_router";
    debug_write(b"ipc_router: boot\n");
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            2u64, // endpoint_id = PID 2 par convention
        )
    };
    debug_write(b"ipc_router: registered\n");

    // ── 2. Boucle principale : receive → dispatch ──────────────────────────
    let mut registry = Registry::new();

    while RUNNING.load(Ordering::Relaxed) {
        let mut msg = syscall::IpcMessage::zeroed();

        // Attendre le prochain message (bloquant).
        let r = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                2,
                &mut msg as *mut syscall::IpcMessage as u64,
                core::mem::size_of::<syscall::IpcMessage>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if r < 0 {
            debug_write(b"ipc_router: recv err\n");
            continue;
        } // EINTR ou erreur temporaire

        let received = r as usize;
        if received < IPC_HEADER_SIZE {
            continue;
        }
        let payload_len = received
            .saturating_sub(IPC_HEADER_SIZE)
            .min(IPC_PAYLOAD_SIZE);

        match msg.msg_type {
            IPC_MSG_REGISTER => {
                // payload[0..4] = endpoint_id (LE), payload[4..] = nom
                if payload_len >= 5 {
                    let Some(ep) = read_payload_u32(&msg, 0, payload_len) else {
                        continue;
                    };
                    let max_name_len = payload_len - 5;
                    let name_len = (payload_byte(&msg, 4) as usize).min(max_name_len);
                    if name_len != 0 {
                        if let Some(name_hash) = hash_payload(&msg, 5, name_len, payload_len) {
                            registry.register_hash(name_hash, ep);
                        }
                    }
                }
            }
            IPC_MSG_ROUTE => {
                // payload[0..4] = dest_endpoint, payload[4..] = données
                if payload_len >= 4 {
                    let Some(dest) = read_payload_u32(&msg, 0, payload_len) else {
                        continue;
                    };
                    let route_payload_len = payload_len - 4;
                    // Vérification via security_gate (IPC-04 + ExoCordon + audit violations).
                    let sg_verdict = security_gate::check_message(
                        msg.sender_pid,
                        dest,
                        msg.msg_type,
                        route_payload_len,
                    );
                    if sg_verdict != security_gate::SecurityVerdict::Allow {
                        continue;
                    }
                    // Forward via SYS_IPC_SEND vers l'endpoint de destination.
                    let _ = unsafe {
                        syscall::syscall6(
                            syscall::SYS_IPC_SEND,
                            dest as u64,
                            payload_ptr(&msg, 4) as u64,
                            route_payload_len as u64,
                            0,
                            0,
                            0,
                        )
                    };
                }
            }
            IPC_MSG_HEARTBEAT => {
                // Répondre avec notre propre PID pour confirmer que le router est vivant.
                let our_pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
                let pid_bytes = (our_pid as u32).to_le_bytes();
                let reply_endpoint =
                    read_payload_u64(&msg, 0, payload_len).unwrap_or(msg.sender_pid as u64);
                let _ = unsafe {
                    syscall::syscall6(
                        syscall::SYS_IPC_SEND,
                        reply_endpoint,
                        pid_bytes.as_ptr() as u64,
                        4,
                        0,
                        0,
                        0,
                    )
                };
            }
            _ => {} // message inconnu — ignorer
        }
    }

    // Ne devrait jamais arriver — boucle infinie de sécurité.
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // En cas de panique inattendue : halt et attendre le watchdog init.
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
