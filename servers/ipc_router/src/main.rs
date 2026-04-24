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
use exo_syscall_abi as syscall;

mod exocordon;
mod security_gate;

/// Registre d'endpoints : max 64 services simultanés.
/// Chaque entrée = (nom hash 32-bit, endpoint_id 32-bit).
struct Registry {
    names: [u32; 64],
    endpoints: [u32; 64],
    count: AtomicU32,
}

impl Registry {
    const fn new() -> Self {
        Self {
            names: [0u32; 64],
            endpoints: [0u32; 64],
            count: AtomicU32::new(0),
        }
    }

    /// Hash FNV-32 du nom du service.
    fn hash_name(name: &[u8]) -> u32 {
        let mut h: u32 = 2166136261;
        let mut i = 0usize;
        while i < name.len() {
            h = h.wrapping_mul(16777619).wrapping_add(name[i] as u32);
            i += 1;
        }
        h
    }

    /// Enregistre un endpoint. Retourne false si la table est pleine.
    fn register(&mut self, name: &[u8], endpoint: u32) -> bool {
        let h = Self::hash_name(name);
        let n = self.count.load(Ordering::Relaxed) as usize;
        if n >= 64 {
            return false;
        }
        self.names[n] = h;
        self.endpoints[n] = endpoint;
        self.count.store((n + 1) as u32, Ordering::Release);
        true
    }

    /// Résout un nom → endpoint_id. Retourne None si inconnu.
    #[allow(dead_code)]
    fn resolve(&self, name: &[u8]) -> Option<u32> {
        let h = Self::hash_name(name);
        let n = self.count.load(Ordering::Acquire) as usize;
        let mut i = 0usize;
        while i < n {
            if self.names[i] == h {
                return Some(self.endpoints[i]);
            }
            i += 1;
        }
        None
    }
}

/// Message IPC reçu du kernel (128 bytes max).
#[repr(C)]
struct IpcMessage {
    sender_pid: u32,
    msg_type: u32,
    /// 0 = REGISTER, 1 = ROUTE, 2 = HEARTBEAT
    payload: [u8; 120],
}

const IPC_MSG_REGISTER: u32 = 0;
const IPC_MSG_ROUTE: u32 = 1;
const IPC_MSG_HEARTBEAT: u32 = 2;
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = syscall::ETIMEDOUT;

// --- Globals no_std (pas de heap) ---
static RUNNING: AtomicBool = AtomicBool::new(true);
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 1. Se déclarer auprès du kernel comme IPC router ──────────────────
    // SYS_IPC_REGISTER(b"ipc_router", endpoint_id=2)
    let name = b"ipc_router";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            2u64, // endpoint_id = PID 2 par convention
        )
    };

    // ── 2. Boucle principale : receive → dispatch ──────────────────────────
    let mut registry = Registry::new();
    let mut msg = IpcMessage {
        sender_pid: 0,
        msg_type: 0,
        payload: [0u8; 120],
    };

    while RUNNING.load(Ordering::Relaxed) {
        // Attendre le prochain message (bloquant).
        let r = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_RECV,
                &mut msg as *mut IpcMessage as u64,
                core::mem::size_of::<IpcMessage>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if r < 0 {
            continue;
        } // EINTR ou erreur temporaire

        match msg.msg_type {
            IPC_MSG_REGISTER => {
                // payload[0..4] = endpoint_id (LE), payload[4..] = nom
                if msg.payload.len() >= 5 {
                    let ep = u32::from_le_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]);
                    let name_len = msg.payload[4] as usize;
                    let name_end = 5usize.saturating_add(name_len).min(msg.payload.len());
                    let svc_name = &msg.payload[5..name_end];
                    registry.register(svc_name, ep);
                }
            }
            IPC_MSG_ROUTE => {
                // payload[0..4] = dest_endpoint, payload[4..] = données
                if msg.payload.len() >= 4 {
                    let dest = u32::from_le_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]);
                    // Vérification via security_gate (IPC-04 + ExoCordon + audit violations).
                    let sg_verdict = security_gate::check_message(
                        msg.sender_pid,
                        dest,
                        msg.msg_type,
                        msg.payload.len().saturating_sub(4),
                    );
                    if sg_verdict != security_gate::SecurityVerdict::Allow {
                        continue;
                    }
                    // Forward via SYS_IPC_SEND vers l'endpoint de destination.
                    let _ = unsafe {
                        syscall::syscall6(
                            syscall::SYS_IPC_SEND,
                            dest as u64,
                            msg.payload[4..].as_ptr() as u64,
                            (msg.payload.len() - 4) as u64,
                            msg.sender_pid as u64,
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
                let _ = unsafe {
                    syscall::syscall6(
                        syscall::SYS_IPC_SEND,
                        msg.sender_pid as u64,
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
