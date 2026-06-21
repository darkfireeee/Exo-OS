//! # xchacha20 — XChaCha20-Poly1305 AEAD pour crypto_server (SRV-04)
//!
//! Wrapper autour de la crate `chacha20poly1305` (RustCrypto, validée IETF).
//!
//! ## Règle SRV-CRYPTO-01
//! Aucune implémentation cryptographique from-scratch dans les serveurs Ring 1.
//! Toutes les primitives sont déléguées aux crates RustCrypto validées.
//!
//! ## Format AEAD : XChaCha20-Poly1305 (RFC 8439 + extension nonce 192 bits)
//! `encrypt(key[32], nonce[24], plaintext, aad) → ciphertext || tag[16]`
//! `decrypt(key[32], nonce[24], ciphertext || tag[16], aad) → plaintext | Err`
//!
//! ## Gestion du nonce
//! Un compteur AtomicU64 est incrémenté à chaque chiffrement.
//! Les 8 premiers octets du nonce[24] proviennent de ce compteur (LE),
//! les 8 suivants d'un sel aléatoire fixé à l'init (via SYS_GETRANDOM),
//! les 8 derniers sont à zéro.
//! Cette construction garantit l'unicité des nonces par session.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chacha20poly1305::{
    aead::{AeadInPlace, KeyInit},
    XChaCha20Poly1305,
};
use exo_syscall_abi as syscall;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Taille de la clé XChaCha20-Poly1305 (256 bits).
pub const KEY_SIZE: usize = 32;
/// Taille du nonce XChaCha20 (192 bits).
pub const NONCE_SIZE: usize = 24;
/// Taille du tag d'authentification Poly1305 (128 bits).
pub const TAG_SIZE: usize = 16;

// ── Nonce counter ─────────────────────────────────────────────────────────────

/// Compteur monotone de nonce (8 premiers octets du nonce[24]).
/// Chaque appel à `xchacha20_seal` l'incrémente atomiquement.
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Sel fixé à l'init (octets [8..16] du nonce[24]).
/// Initialisé via SYS_GETRANDOM au démarrage. Zéro avant init.
static NONCE_SALT_LO: AtomicU64 = AtomicU64::new(0);
static NONCE_SALT_HI: AtomicU64 = AtomicU64::new(0);
static XCHACHA_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Lit 8 octets aléatoires via SYS_GETRANDOM (CSPRNG de l'OS — source primaire).
///
/// SÉCURITÉ : SYS_GETRANDOM est réessayé (échec transitoire rare) AVANT toute
/// dégradation. Le repli n'est PAS une « source aléatoire » présentée comme telle
/// (ce serait de la fausse sécurité) : c'est un mélange dégradé explicite de
/// plusieurs lectures TSC espacées + adresse de pile, utilisé uniquement comme
/// **sel de nonce**. L'unicité des nonces INTRA-session est de toute façon garantie
/// par le compteur monotone `NONCE_COUNTER` (ce sel n'ajoute que de l'unicité
/// inter-sessions). Aucune clé/secret n'est dérivé de cette valeur.
fn getrandom_u64() -> u64 {
    for _ in 0..16 {
        let mut buf = [0u8; 8];
        let ret: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                in("rax")  syscall::SYS_GETRANDOM,
                in("rdi")  buf.as_mut_ptr() as u64,
                in("rsi")  8u64,
                in("rdx")  0u64,             // flags = 0 (GRND_DEFAULT)
                lateout("rax") ret,
                out("rcx") _, out("r11") _,
                options(nostack),
            );
        }
        if ret == 8 {
            return u64::from_le_bytes(buf);
        }
    }

    // Repli DÉGRADÉ (ne devrait jamais arriver) : plusieurs lectures TSC espacées
    // par PAUSE (jitter d'horloge) mixées + adresse de pile.
    let mut acc: u64 = 0;
    let mut probe = [0u8; 8];
    for _ in 0..8 {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
            core::arch::asm!("pause", options(nostack, nomem));
        }
        acc = acc.rotate_left(13) ^ (((hi as u64) << 32) | lo as u64);
    }
    acc ^ ((&mut probe as *mut [u8; 8] as usize) as u64).rotate_left(17)
}

/// Initialise le sous-système XChaCha20-Poly1305.
/// Doit être appelé une fois depuis `_start()`.
pub fn xchacha20_init() {
    // Charger le sel depuis RDRAND via SYS_GETRANDOM
    let salt_lo = getrandom_u64();
    let salt_hi = getrandom_u64();
    NONCE_SALT_LO.store(salt_lo, Ordering::Release);
    NONCE_SALT_HI.store(salt_hi, Ordering::Release);
    XCHACHA_INITIALIZED.store(true, Ordering::Release);
}

/// Reseed post-Phoenix : mélange une nouvelle entropie dans le sel des nonces
/// sans jamais réutiliser un espace de compteur déjà exposé.
pub fn xchacha20_reseed(entropy: u64) {
    if !XCHACHA_INITIALIZED.load(Ordering::Acquire) {
        xchacha20_init();
    }

    let counter = NONCE_COUNTER.fetch_add(1_000_000, Ordering::AcqRel);
    let old_lo = NONCE_SALT_LO.load(Ordering::Acquire);
    let old_hi = NONCE_SALT_HI.load(Ordering::Acquire);

    let mixed_lo = old_lo ^ entropy ^ counter;
    let mixed_hi = old_hi ^ entropy.rotate_left(17) ^ counter.rotate_left(31);

    NONCE_SALT_LO.store(mixed_lo, Ordering::Release);
    NONCE_SALT_HI.store(mixed_hi, Ordering::Release);
    core::sync::atomic::fence(Ordering::SeqCst);
}

/// Construit le nonce[24] à partir du compteur et du sel.
fn build_nonce() -> [u8; NONCE_SIZE] {
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let salt_lo = NONCE_SALT_LO.load(Ordering::Relaxed);
    let salt_hi = NONCE_SALT_HI.load(Ordering::Relaxed);

    let mut nonce = [0u8; NONCE_SIZE];
    nonce[0..8].copy_from_slice(&counter.to_le_bytes());
    nonce[8..16].copy_from_slice(&salt_lo.to_le_bytes());
    nonce[16..24].copy_from_slice(&salt_hi.to_le_bytes());
    nonce
}

// ── API publique ──────────────────────────────────────────────────────────────

/// Chiffre `plaintext` avec XChaCha20-Poly1305.
///
/// # Arguments
/// * `key`       — Clé de chiffrement 32 octets.
/// * `plaintext` — Données à chiffrer.
/// * `aad`       — Additional Authenticated Data (peut être vide).
/// * `out_nonce` — Buffer [24] pour recevoir le nonce utilisé.
/// * `out_buf`   — Buffer de sortie pour `ciphertext || tag` (taille ≥ plaintext.len() + 16).
///
/// # Retourne
/// Longueur du buffer chiffré (plaintext.len() + TAG_SIZE), ou 0 en cas d'erreur.
pub fn xchacha20_seal(
    key: &[u8; KEY_SIZE],
    plaintext: &[u8],
    aad: &[u8],
    out_nonce: &mut [u8; NONCE_SIZE],
    out_buf: &mut [u8],
) -> usize {
    let needed = plaintext.len() + TAG_SIZE;
    if out_buf.len() < needed {
        return 0;
    }

    let cipher = match XChaCha20Poly1305::new_from_slice(key) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let nonce_bytes = build_nonce();
    *out_nonce = nonce_bytes;

    // Copier le plaintext dans le buffer de sortie
    out_buf[..plaintext.len()].copy_from_slice(plaintext);

    let nonce = chacha20poly1305::XNonce::from_slice(&nonce_bytes);
    match cipher.encrypt_in_place_detached(nonce, aad, &mut out_buf[..plaintext.len()]) {
        Ok(tag) => {
            out_buf[plaintext.len()..needed].copy_from_slice(tag.as_slice());
            needed
        }
        Err(_) => 0,
    }
}

/// Déchiffre `ciphertext || tag` avec XChaCha20-Poly1305.
///
/// # Arguments
/// * `key`        — Clé de déchiffrement 32 octets.
/// * `nonce`      — Nonce 24 octets (reçu avec le message).
/// * `ciphertext` — Buffer `ciphertext || tag[16]` (taille ≥ TAG_SIZE).
/// * `aad`        — Additional Authenticated Data (doit correspondre à l'AAD du seal).
/// * `out_buf`    — Buffer de sortie pour le plaintext (taille ≥ ciphertext.len() - TAG_SIZE).
///
/// # Retourne
/// Longueur du plaintext, ou 0 si l'authentification échoue.
pub fn xchacha20_open(
    key: &[u8; KEY_SIZE],
    nonce: &[u8; NONCE_SIZE],
    ciphertext: &[u8],
    aad: &[u8],
    out_buf: &mut [u8],
) -> usize {
    if ciphertext.len() < TAG_SIZE {
        return 0;
    }
    let pt_len = ciphertext.len() - TAG_SIZE;
    if out_buf.len() < pt_len {
        return 0;
    }

    let cipher = match XChaCha20Poly1305::new_from_slice(key) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // Copier ciphertext (sans tag) dans le buffer de sortie
    out_buf[..pt_len].copy_from_slice(&ciphertext[..pt_len]);

    let tag = chacha20poly1305::Tag::from_slice(&ciphertext[pt_len..]);
    let nonce_obj = chacha20poly1305::XNonce::from_slice(nonce);

    match cipher.decrypt_in_place_detached(nonce_obj, aad, &mut out_buf[..pt_len], tag) {
        Ok(()) => pt_len,
        Err(_) => {
            // Effacer le buffer en cas d'échec d'authentification (défense en profondeur)
            out_buf[..pt_len].fill(0);
            0
        }
    }
}
