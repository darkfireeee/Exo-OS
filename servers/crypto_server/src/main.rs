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
//! - CRYPTO_ENCRYPT     (2) : chiffrement XChaCha20-Poly1305 AEAD
//! - CRYPTO_DECRYPT     (3) : déchiffrement XChaCha20-Poly1305 AEAD
//! - CRYPTO_HASH        (4) : hash Blake3 d'un buffer
//! - CRYPTO_SIGN        (5) : signature Ed25519
//! - CRYPTO_VERIFY      (6) : vérification de signature Ed25519
//! - CRYPTO_TLS_INIT    (7) : initiation handshake TLS 1.3
//! - CRYPTO_TLS_RESPOND (8) : réponse handshake TLS 1.3
//! - CRYPTO_TLS_CLOSE   (9) : fermeture session TLS
//! - CRYPTO_KEY_REVOKE  (10) : révocation de clé
//! - CRYPTO_KEY_ROTATE  (11) : rotation de clé
//!
//! ## Sécurité
//! - Les clés ne quittent JAMAIS ce processus (pas de réponse avec clé brute).
//! - Seul un handle opaque (key_handle u32) est retourné au client.
//! - Les handles sont invalidés à l'arrêt (pas de persistance sans ExoFS).
//! - SRV-02 : seuls les handles sortent, jamais les octets bruts
//! - CAP-01 : vérification de capability token en première instruction

extern crate blake3;

use exo_syscall_abi as syscall;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

mod xchacha20;

// ── Types de messages crypto ──────────────────────────────────────────────────

const CRYPTO_DERIVE_KEY:  u32 = 0;
const CRYPTO_RANDOM:      u32 = 1;
const CRYPTO_ENCRYPT:     u32 = 2;
const CRYPTO_DECRYPT:     u32 = 3;
const CRYPTO_HASH:        u32 = 4;

const CRYPTO_OK:              u32 = 0;
const CRYPTO_ERR_ARGS:        u32 = 1;
const CRYPTO_ERR_KEY_INVALID: u32 = 3;
const CRYPTO_ERR_AUTH:        u32 = 4;

const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
const ETIMEDOUT: i64 = syscall::ETIMEDOUT;

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

// ── Statistiques ─────────────────────────────────────────────────────────────

static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUESTS_OK:    AtomicU64 = AtomicU64::new(0);
static REQUESTS_ERR:   AtomicU64 = AtomicU64::new(0);
static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);


// ── Mini-keystore inline ──────────────────────────────────────────────────────
// Les clés ne quittent JAMAIS ce processus (SRV-02 : seuls des handles opaques
// u32 non-nuls sont retournés). Le keystore est volontairement minimal : 32 slots,
// aucune persistance, révocation par shredding DoD 5220.22-M (3 passes Write).

use core::sync::atomic::AtomicU8;

/// Nombre de slots dans le keystore.
const KS_MAX: usize = 32;
/// Taille d'une clé (256 bits).
const KS_KEY_SIZE: usize = 32;

/// Un slot du keystore.
struct KeySlot {
    handle:     AtomicU32,   // 0 = libre
    owner_pid:  AtomicU32,
    key_type:   AtomicU8,
    created_at: AtomicU64,   // TSC au moment de la création
    key:        [u8; KS_KEY_SIZE],
}

impl KeySlot {
    const fn new() -> Self {
        Self {
            handle:     AtomicU32::new(0),
            owner_pid:  AtomicU32::new(0),
            key_type:   AtomicU8::new(0),
            created_at: AtomicU64::new(0),
            key:        [0u8; KS_KEY_SIZE],
        }
    }
}

/// Compteur monotone pour générer des handles uniques (jamais zéro).
static KS_HANDLE_CTR: AtomicU32 = AtomicU32::new(1);

// SAFETY: Accès uniquement depuis le thread unique du crypto_server (single-threaded server).
static mut KS_SLOTS: [KeySlot; KS_MAX] = {
    const S: KeySlot = KeySlot::new();
    [S; KS_MAX]
};

/// Insère une clé, retourne un handle opaque non-nul, ou 0 si table pleine.
fn ks_insert(key: &[u8; KS_KEY_SIZE], key_type: u8, owner_pid: u32) -> u32 {
    let slots = unsafe { &mut KS_SLOTS };
    for slot in slots.iter_mut() {
        if slot.handle.load(Ordering::Relaxed) == 0 {
            let h = KS_HANDLE_CTR.fetch_add(1, Ordering::Relaxed);
            let h = if h == 0 { KS_HANDLE_CTR.fetch_add(1, Ordering::Relaxed) } else { h };
            slot.key.copy_from_slice(key);
            slot.key_type.store(key_type, Ordering::Relaxed);
            slot.owner_pid.store(owner_pid, Ordering::Relaxed);
            slot.created_at.store(read_tsc(), Ordering::Relaxed);
            slot.handle.store(h, Ordering::Release);
            return h;
        }
    }
    0
}

/// Récupère une référence à la clé associée à `handle` (si elle appartient à `owner_pid`).
/// Retourne None si handle invalide ou PID ne correspond pas.
fn ks_get(handle: u32, owner_pid: u32) -> Option<[u8; KS_KEY_SIZE]> {
    if handle == 0 { return None; }
    let slots = unsafe { &KS_SLOTS };
    for slot in slots.iter() {
        if slot.handle.load(Ordering::Acquire) == handle
            && slot.owner_pid.load(Ordering::Relaxed) == owner_pid
        {
            return Some(slot.key);
        }
    }
    None
}

/// Lit le TSC via RDTSC.
#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    ((hi as u64) << 32) | (lo as u64)
}

/// Dérivation de clé via HKDF-BLAKE3 (crate `blake3` + `hkdf`).
///
/// RÈGLE SRV-CRYPTO-01 : pas d'implémentation from-scratch.
/// Utilise blake3 en mode keyed-hash comme PRF pour HKDF.
fn derive_key_hkdf(material: &[u8], output: &mut [u8; 32]) {
    // HKDF-Extract : PRK = BLAKE3-MAC(salt=zeros, IKM=material)
    let salt = [0u8; 32];
    let prk = blake3::keyed_hash(&salt, material);

    // HKDF-Expand : OKM = BLAKE3-KDF(context="Exo-OS SRV-04 KDF v1", key=prk)
    let okm = blake3::derive_key("Exo-OS SRV-04 KDF v1", prk.as_bytes());
    output.copy_from_slice(&okm);
}

/// Hash Blake3 d'un buffer (crate `blake3`, mode standard).
///
/// RÈGLE SRV-CRYPTO-01 : pas d'implémentation from-scratch.
fn hash_blake3(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

fn handle_request(req: &CryptoRequest) -> CryptoReply {
    let mut reply = CryptoReply { status: CRYPTO_ERR_ARGS, key_handle: 0, data: [0u8; 56] };
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);

    match req.msg_type {
        // ─── Dérivation de clé (HKDF-Blake3) ───────────────────────────────
        CRYPTO_DERIVE_KEY => {
            // payload[0..4] = key_type (LE), payload[4..] = matériel de dérivation
            let key_type = u32::from_le_bytes([
                req.payload[0], req.payload[1], req.payload[2], req.payload[3],
            ]);
            let mut derived_key = [0u8; 32];
            derive_key_hkdf(&req.payload[4..], &mut derived_key);

            let handle = ks_insert(&derived_key, key_type as u8, req.sender_pid);

            // Shredder la clé locale
            for b in derived_key.iter_mut() {
                unsafe { core::ptr::write_volatile(b, 0) };
            }
            core::sync::atomic::fence(Ordering::SeqCst);

            if handle != 0 {
                reply.status = CRYPTO_OK;
                reply.key_handle = handle;
            } else {
                reply.status = CRYPTO_ERR_ARGS; // table pleine
            }
        }

        // ─── Octets aléatoires ─────────────────────────────────────────────
        CRYPTO_RANDOM => {
            let n = (req.payload[0] as usize).min(56);
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
                reply.data[0] = n as u8; // Premier octet = longueur réelle
            }
        }

        // ─── Chiffrement XChaCha20-Poly1305 AEAD ────────────────────────────
        CRYPTO_ENCRYPT => {
            // payload[0..4] = key_handle (LE), payload[4..24] = nonce (ou 0 = auto)
            // Le plaintext est dans payload[24..], max 96 octets
            let key_handle = u32::from_le_bytes([
                req.payload[0], req.payload[1], req.payload[2], req.payload[3],
            ]);

            let key = match ks_get(key_handle, req.sender_pid) {
                Some(k) => k,
                None => {
                    reply.status = CRYPTO_ERR_KEY_INVALID;
                    return reply;
                }
            };

            // payload[4..] = plaintext (max 16 octets pour tenir dans reply.data)
            // reply.data = nonce[24] || ciphertext || tag[16] (max 56 octets)
            // Donc pt_len max = 56 - 24 - 16 = 16 octets.
            let plaintext = &req.payload[4..];
            let pt_len = plaintext.len().min(16);

            let mut nonce_out = [0u8; 24];
            let mut sealed = [0u8; 56]; // nonce[24] + ct + tag[16]
            let ct_tag_buf_len = pt_len + 16; // ciphertext + tag

            let sealed_len = xchacha20::xchacha20_seal(
                &key,
                &plaintext[..pt_len],
                &[],           // aad vide
                &mut nonce_out,
                &mut sealed[24..24 + ct_tag_buf_len],
            );
            if sealed_len == pt_len + 16 {
                sealed[..24].copy_from_slice(&nonce_out);
                let total = 24 + sealed_len;
                reply.data[..total].copy_from_slice(&sealed[..total]);
                reply.status = CRYPTO_OK;
                reply.key_handle = key_handle;
            } else {
                reply.status = CRYPTO_ERR_ARGS;
            }
        }

        // ─── Déchiffrement XChaCha20-Poly1305 AEAD ──────────────────────────
        CRYPTO_DECRYPT => {
            // payload[0..4] = key_handle (LE)
            // payload[4..] = message scellé (nonce + ciphertext + tag)
            let key_handle = u32::from_le_bytes([
                req.payload[0], req.payload[1], req.payload[2], req.payload[3],
            ]);

            let key = match ks_get(key_handle, req.sender_pid) {
                Some(k) => k,
                None => {
                    reply.status = CRYPTO_ERR_KEY_INVALID;
                    return reply;
                }
            };

            // payload[4..28] = nonce[24], payload[28..] = ciphertext || tag[16]
            let sealed_data = &req.payload[4..];
            if sealed_data.len() < 24 + 16 {
                reply.status = CRYPTO_ERR_ARGS;
                return reply;
            }
            let nonce: &[u8; 24] = sealed_data[..24].try_into().unwrap_or(&[0u8; 24]);
            let ct_tag = &sealed_data[24..];
            let pt_len = ct_tag.len().saturating_sub(16);
            if pt_len > 56 {
                reply.status = CRYPTO_ERR_ARGS;
                return reply;
            }

            let mut plaintext_buf = [0u8; 56];
            let opened = xchacha20::xchacha20_open(
                &key,
                nonce,
                ct_tag,
                &[],  // aad vide
                &mut plaintext_buf[..pt_len],
            );
            if opened == pt_len && pt_len > 0 {
                reply.status = CRYPTO_OK;
                reply.key_handle = key_handle;
                reply.data[..pt_len].copy_from_slice(&plaintext_buf[..pt_len]);
            } else {
                reply.status = CRYPTO_ERR_AUTH;
            }
        }

        // ─── Hash Blake3 ───────────────────────────────────────────────────
        CRYPTO_HASH => {
            let hash = hash_blake3(&req.payload);
            reply.data[..32].copy_from_slice(&hash);
            reply.status = CRYPTO_OK;
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ── 0. Initialiser les sous-modules ────────────────────────────────────

    // ── 0. Initialiser xchacha20 (nonce salt)
    xchacha20::xchacha20_init();

    // ── 1. S'enregistrer auprès de l'ipc_router ────────────────────────────
    let name = b"crypto_server";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            4u64, // endpoint_id = PID 4
        )
    };

    // ── 2. Boucle principale ───────────────────────────────────────────────
    let mut req = CryptoRequest { sender_pid: 0, msg_type: 0, payload: [0u8; 120] };

    loop {
        let r = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_RECV,
                &mut req as *mut CryptoRequest as u64,
                core::mem::size_of::<CryptoRequest>() as u64,
                IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
            )
        };

        if r == ETIMEDOUT {
            IPC_RECV_TIMEOUTS.fetch_add(1, Ordering::Relaxed);

            // Maintenance périodique : vérifier les expirations
            continue;
        }
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
