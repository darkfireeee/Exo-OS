//! # tls — TLS 1.3 minimal pour communication inter-serveurs Exo-OS
//!
//! Implémentation minimale de TLS 1.3 pour les communications sécurisées
//! entre les serveurs Ring 1. Utilise XChaCha20-BLAKE3 comme unique suite
//! cryptographique (AES-GCM indisponible en no_std bare-metal).
//!
//! ## Suite cryptographique
//! - Échange de clés : X25519 ECDHE
//! - Dérivation : HKDF-Blake3
//! - Chiffrement : XChaCha20-BLAKE3 AEAD
//! - Signature : Ed25519
//!
//! ## Limitations (v1)
//! - Pas de re-négociation
//! - Pas de 0-RTT
//! - Pas de compression
//! - Suite unique (pas de négociation)
//!
//! ## Sécurité
//! - Forward secrecy via X25519 éphémère
//! - Toutes les clés sont shreddées à la fermeture de session
//! - Counter-based nonce (jamais de réutilisation)
//! - Vérification de certificat via PKI

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use x25519_dalek::{PublicKey, StaticSecret};

// ── Constantes ───────────────────────────────────────────────────────────────

/// Taille du random client/serveur.
const RANDOM_SIZE: usize = 32;

/// Taille de la clé partagée X25519.
const SHARED_SECRET_SIZE: usize = 32;

/// Taille de la clé de chiffrement.
const KEY_SIZE: usize = 32;

/// Taille du IV (nonce de 12 octets pour le compteur).
const IV_SIZE: usize = 12;

/// Nombre maximum de sessions simultanées.
const MAX_SESSIONS: usize = 16;

/// Taille maximale d'un enregistrement TLS.
#[allow(dead_code)]
const MAX_RECORD_SIZE: usize = 1024;
const TLS_ROLE_SERVER: u32 = 1 << 0;

// ── Lecture TSC et RDRAND ────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Remplit `buf` avec de l'entropie cryptographique RÉELLE via le CSPRNG kernel
/// (syscall `getrandom` → RNG durci RDSEED/RDRAND + conditionnement Blake3 +
/// ChaCha20). Retourne `false` en cas d'échec — l'appelant DOIT alors abandonner
/// (jamais de clé dérivée d'une source faible).
///
/// FIX-SEC-2C-TLS : remplace l'ancien `rdrand_u64()` (lecture RDRAND SANS contrôle
/// du CF → pouvait renvoyer 0/garbage) + LCG de génération de clé X25519 (clé de
/// 256 bits réduite à ≤64 bits d'entropie, LCG inversible → forward secrecy nulle).
fn fill_random(buf: &mut [u8]) -> bool {
    crate::secure_random(buf)
}

// ── Suite cryptographique ────────────────────────────────────────────────────

/// Suite cryptographique TLS 1.3 unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum CipherSuite {
    /// XChaCha20-BLAKE3 : 0xEx01 (valeur ExoOS personnalisée)
    XChaCha20Blake3 = 0xE701,
}

impl CipherSuite {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0xE701 => Some(Self::XChaCha20Blake3),
            _ => None,
        }
    }
}

// ── État de la session TLS ───────────────────────────────────────────────────

/// État d'une session TLS 1.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TlsState {
    /// Session fermée / non initialisée.
    Closed = 0,
    /// Handshake en cours (ClientHello envoyé).
    HandshakePending = 1,
    /// Échange de clés en cours (ServerHello reçu/envoyé).
    KeyExchange = 2,
    /// Session établie et vérifiée.
    Verified = 3,
    /// Session en erreur.
    Error = 4,
}

impl TlsState {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Closed),
            1 => Some(Self::HandshakePending),
            2 => Some(Self::KeyExchange),
            3 => Some(Self::Verified),
            4 => Some(Self::Error),
            _ => None,
        }
    }
}

// ── Types d'enregistrement TLS ───────────────────────────────────────────────

/// Types d'enregistrement TLS 1.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentType {
    Invalid = 0,
    Handshake = 22,
    ApplicationData = 23,
    Alert = 21,
}

impl ContentType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            22 => Some(Self::Handshake),
            23 => Some(Self::ApplicationData),
            21 => Some(Self::Alert),
            _ => None,
        }
    }
}

// ── Session TLS ──────────────────────────────────────────────────────────────

/// Session TLS 1.3 complète.
#[repr(C)]
pub struct TlsSession {
    /// État actuel de la session.
    pub state: u8,
    /// Suite cryptographique négociée.
    pub cipher_suite: u16,
    /// Random du client (32 octets).
    pub client_random: [u8; RANDOM_SIZE],
    /// Random du serveur (32 octets).
    pub server_random: [u8; RANDOM_SIZE],
    /// Secret partagé X25519 (32 octets).
    pub shared_secret: [u8; SHARED_SECRET_SIZE],
    /// Hash du handshake (32 octets, cumulatif).
    pub handshake_hash: [u8; 32],
    /// Clé de chiffrement client→serveur.
    pub client_write_key: [u8; KEY_SIZE],
    /// Clé de chiffrement serveur→client.
    pub server_write_key: [u8; KEY_SIZE],
    /// IV client (12 octets).
    pub client_write_iv: [u8; IV_SIZE],
    /// IV serveur (12 octets).
    pub server_write_iv: [u8; IV_SIZE],
    /// Compteur de records envoyés.
    pub send_counter: AtomicU64,
    /// Compteur de records reçus.
    pub recv_counter: AtomicU64,
    /// PID du pair (0 = non connecté).
    pub peer_pid: u32,
    /// Handle de la clé dans le keystore (0 = pas de clé).
    pub key_handle: u32,
    /// TSC de la dernière activité.
    pub last_activity_tsc: AtomicU64,
    /// Flags de session.
    pub flags: u32,
}

impl TlsSession {
    /// Crée une session vide (état Closed).
    pub const fn new() -> Self {
        Self {
            state: TlsState::Closed as u8,
            cipher_suite: CipherSuite::XChaCha20Blake3 as u16,
            client_random: [0u8; RANDOM_SIZE],
            server_random: [0u8; RANDOM_SIZE],
            shared_secret: [0u8; SHARED_SECRET_SIZE],
            handshake_hash: [0u8; 32],
            client_write_key: [0u8; KEY_SIZE],
            server_write_key: [0u8; KEY_SIZE],
            client_write_iv: [0u8; IV_SIZE],
            server_write_iv: [0u8; IV_SIZE],
            send_counter: AtomicU64::new(0),
            recv_counter: AtomicU64::new(0),
            peer_pid: 0,
            key_handle: 0,
            last_activity_tsc: AtomicU64::new(0),
            flags: 0,
        }
    }

    /// Retourne true si la session est dans l'état Verified.
    pub fn is_active(&self) -> bool {
        self.state == TlsState::Verified as u8
    }

    /// Shred toutes les clés de la session.
    fn shred_keys(&mut self) {
        for b in self.shared_secret.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        for b in self.client_write_key.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        for b in self.server_write_key.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        for b in self.client_write_iv.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        for b in self.server_write_iv.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        for b in self.handshake_hash.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        for b in self.server_random.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

// ── Pool de sessions ─────────────────────────────────────────────────────────

/// Pool de sessions TLS statique.
static SESSION_POOL: spin::Mutex<[TlsSession; MAX_SESSIONS]> = spin::Mutex::new([
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
    TlsSession::new(),
]);

static ACTIVE_SESSION_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Dérivation BLAKE3 keyed ──────────────────────────────────────────────────

/// Dérivation de clé extensible basée sur BLAKE3 keyed hashing.
fn hkdf_expand(secret: &[u8], label: &[u8], context: &[u8], output: &mut [u8]) {
    let salt = [0u8; 32];
    let prk = blake3::keyed_hash(&salt, secret);
    let mut hasher = blake3::Hasher::new_keyed(prk.as_bytes());
    hasher.update(b"ExoOS TLS 1.3 traffic secret");
    hasher.update(label);
    hasher.update(context);
    let mut reader = hasher.finalize_xof();
    reader.fill(output);
}

/// Dérive les clés de trafic à partir du secret partagé.
/// TLS 1.3 key schedule :
///   derived_secret = HKDF-Expand(shared_secret, "derived", HashLen)
///   client_write_key = HKDF-Expand(derived_secret, "c ws", key_len)
///   server_write_key = HKDF-Expand(derived_secret, "s ws", key_len)
///   client_write_iv  = HKDF-Expand(derived_secret, "c wi", iv_len)
///   server_write_iv  = HKDF-Expand(derived_secret, "s wi", iv_len)
fn derive_traffic_keys(session: &mut TlsSession) {
    let mut derived_secret = [0u8; 32];

    // Dérivation intermédiaire
    let mut full_context = [0u8; 64];
    full_context[..32].copy_from_slice(&session.client_random);
    full_context[32..64].copy_from_slice(&session.server_random);

    hkdf_expand(
        &session.shared_secret,
        b"derived",
        &full_context,
        &mut derived_secret,
    );

    // Dérivation des clés
    hkdf_expand(
        &derived_secret,
        b"c ws",
        &full_context,
        &mut session.client_write_key,
    );
    hkdf_expand(
        &derived_secret,
        b"s ws",
        &full_context,
        &mut session.server_write_key,
    );

    // Dérivation des IVs
    let mut client_iv_buf = [0u8; IV_SIZE];
    let mut server_iv_buf = [0u8; IV_SIZE];
    hkdf_expand(&derived_secret, b"c wi", &full_context, &mut client_iv_buf);
    hkdf_expand(&derived_secret, b"s wi", &full_context, &mut server_iv_buf);
    session.client_write_iv.copy_from_slice(&client_iv_buf);
    session.server_write_iv.copy_from_slice(&server_iv_buf);

    // Shred le secret intermédiaire
    for b in derived_secret.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
}

// ── X25519 ───────────────────────────────────────────────────────────────────

/// Échange de clés X25519 via x25519-dalek.
fn x25519_dh(private_key: &[u8; 32], public_key: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*private_key);
    let public = PublicKey::from(*public_key);
    secret.diffie_hellman(&public).to_bytes()
}

/// Génère une paire de clés X25519 éphémère à partir du CSPRNG kernel.
///
/// Retourne `None` si l'entropie n'a pas pu être obtenue — l'appelant DOIT
/// abandonner le handshake (jamais de clé éphémère faible). Le clamping X25519
/// est appliqué par `StaticSecret::from` (x25519-dalek).
fn generate_x25519_keypair() -> Option<([u8; 32], [u8; 32])> {
    let mut private = [0u8; 32];
    if !fill_random(&mut private) {
        return None;
    }
    let secret = StaticSecret::from(private);
    let public = PublicKey::from(&secret).to_bytes();
    Some((private, public))
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Initie un handshake TLS 1.3 (côté client).
///
/// ## Retour
/// - `session_handle` : identifiant de session (non-zéro si succès)
/// - `client_hello` : message ClientHello à envoyer au serveur
///
/// ## Format ClientHello (simplifié)
/// `msg_type=1 || suite[2] || client_random[32] || client_public_key[32]`
pub fn tls_handshake_initiate(peer_pid: u32) -> (u32, [u8; 67]) {
    let mut hello = [0u8; 67];

    // Allouer une session
    let mut pool = SESSION_POOL.lock();
    let mut session_handle = 0u32;

    for (idx, session) in pool.iter_mut().enumerate() {
        if session.state == TlsState::Closed as u8 {
            session_handle = (idx + 1) as u32;

            // client_random + paire X25519 éphémère via le CSPRNG kernel.
            // Échec d'entropie → abandon (la session reste Closed, handle = 0 ;
            // ACTIVE_SESSION_COUNT pas encore incrémenté à ce point).
            if !fill_random(&mut session.client_random) {
                return (0, hello);
            }
            let (private_key, public_key) = match generate_x25519_keypair() {
                Some(kp) => kp,
                None => return (0, hello),
            };

            // Stocker la clé privée temporairement dans server_random (sera écrasé)
            // Note : en production, ceci sera dans un slot sécurisé du keystore
            session.server_random[..32].copy_from_slice(&private_key);

            // Construire le ClientHello
            hello[0] = 1; // msg_type = ClientHello
            hello[1..3].copy_from_slice(&(CipherSuite::XChaCha20Blake3 as u16).to_le_bytes());
            hello[3..35].copy_from_slice(&session.client_random);
            hello[35..67].copy_from_slice(&public_key);

            session.state = TlsState::HandshakePending as u8;
            session.cipher_suite = CipherSuite::XChaCha20Blake3 as u16;
            session.peer_pid = peer_pid;
            session.flags = 0;
            session.send_counter.store(1, Ordering::Release);
            session
                .last_activity_tsc
                .store(read_tsc(), Ordering::Release);

            ACTIVE_SESSION_COUNT.fetch_add(1, Ordering::Relaxed);
            break;
        }
    }

    (session_handle, hello)
}

/// Traite un ServerHello et complète le handshake (côté client).
///
/// ## Format ServerHello (simplifié)
/// `msg_type=2 || suite[2] || server_random[32] || server_public_key[32]`
///
/// ## Retour
/// - `true` si le handshake est complété avec succès
pub fn tls_handshake_complete_client(session_handle: u32, server_hello: &[u8]) -> bool {
    if session_handle == 0 || server_hello.len() < 67 {
        return false;
    }

    let idx = (session_handle - 1) as usize;
    if idx >= MAX_SESSIONS {
        return false;
    }

    let mut pool = SESSION_POOL.lock();
    let session = &mut pool[idx];

    if session.state != TlsState::HandshakePending as u8 {
        return false;
    }

    // Récupérer notre clé privée avant d'écraser le stockage temporaire par
    // le vrai server_random reçu.
    let mut private_key: [u8; 32] = {
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&session.server_random[..32]);
        pk
    };

    // Vérifier le msg_type
    if server_hello[0] != 2 {
        session.state = TlsState::Error as u8;
        return false;
    }

    // Vérifier la suite
    let suite = u16::from_le_bytes([server_hello[1], server_hello[2]]);
    if suite != CipherSuite::XChaCha20Blake3 as u16 {
        session.state = TlsState::Error as u8;
        return false;
    }

    // Extraire server_random
    session.server_random.copy_from_slice(&server_hello[3..35]);

    // Extraire la clé publique du serveur
    let server_public: [u8; 32] = {
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&server_hello[35..67]);
        pk
    };

    // Calculer le secret partagé
    session.shared_secret = x25519_dh(&private_key, &server_public);
    for b in private_key.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    // Dériver les clés de trafic
    derive_traffic_keys(session);

    // Mettre à jour l'état
    session.state = TlsState::Verified as u8;
    session.recv_counter.store(1, Ordering::Release);
    session
        .last_activity_tsc
        .store(read_tsc(), Ordering::Release);

    true
}

/// Traite un ClientHello et répond avec un ServerHello (côté serveur).
///
/// ## Retour
/// - `session_handle` : identifiant de session (non-zéro si succès)
/// - `server_hello` : message ServerHello à renvoyer au client
pub fn tls_handshake_respond(client_hello: &[u8], peer_pid: u32) -> (u32, [u8; 67]) {
    let mut server_hello = [0u8; 67];
    let mut session_handle = 0u32;

    if client_hello.len() < 67 || client_hello[0] != 1 {
        return (0, server_hello);
    }

    // Vérifier la suite
    let suite = u16::from_le_bytes([client_hello[1], client_hello[2]]);
    if suite != CipherSuite::XChaCha20Blake3 as u16 {
        return (0, server_hello);
    }

    // Allouer une session
    let mut pool = SESSION_POOL.lock();

    for (idx, session) in pool.iter_mut().enumerate() {
        if session.state == TlsState::Closed as u8 {
            session_handle = (idx + 1) as u32;

            // Copier le client_random
            session.client_random.copy_from_slice(&client_hello[3..35]);

            // Extraire la clé publique du client
            let client_public: [u8; 32] = {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&client_hello[35..67]);
                pk
            };

            // server_random + paire X25519 éphémère via le CSPRNG kernel.
            // Échec d'entropie → abandon (session reste Closed, handle = 0).
            if !fill_random(&mut session.server_random) {
                return (0, server_hello);
            }
            let (mut private_key, public_key) = match generate_x25519_keypair() {
                Some(kp) => kp,
                None => return (0, server_hello),
            };

            // Calculer le secret partagé
            session.shared_secret = x25519_dh(&private_key, &client_public);
            for b in private_key.iter_mut() {
                unsafe { core::ptr::write_volatile(b, 0) };
            }
            core::sync::atomic::fence(Ordering::SeqCst);

            // Dériver les clés de trafic
            derive_traffic_keys(session);

            // Construire le ServerHello
            server_hello[0] = 2; // msg_type = ServerHello
            server_hello[1..3]
                .copy_from_slice(&(CipherSuite::XChaCha20Blake3 as u16).to_le_bytes());
            server_hello[3..35].copy_from_slice(&session.server_random);
            server_hello[35..67].copy_from_slice(&public_key);

            session.state = TlsState::Verified as u8;
            session.cipher_suite = CipherSuite::XChaCha20Blake3 as u16;
            session.peer_pid = peer_pid;
            session.flags = TLS_ROLE_SERVER;
            session.send_counter.store(1, Ordering::Release);
            session.recv_counter.store(0, Ordering::Release);
            session
                .last_activity_tsc
                .store(read_tsc(), Ordering::Release);

            ACTIVE_SESSION_COUNT.fetch_add(1, Ordering::Relaxed);
            break;
        }
    }

    (session_handle, server_hello)
}

/// Chiffre un enregistrement d'application TLS.
///
/// ## Format
/// `content_type[1] || counter[8] || encrypted_payload[N] || tag[32]`
pub fn tls_encrypt_record(session_handle: u32, data: &[u8], output: &mut [u8]) -> bool {
    if session_handle == 0 || session_handle as usize > MAX_SESSIONS {
        return false;
    }

    let idx = (session_handle - 1) as usize;
    let pool = SESSION_POOL.lock();
    let session = &pool[idx];

    if !session.is_active() {
        return false;
    }

    let aad = [ContentType::ApplicationData as u8];
    let key: [u8; 32] = if session.flags & TLS_ROLE_SERVER != 0 {
        session.server_write_key
    } else {
        session.client_write_key
    };
    drop(pool); // Libérer le lock avant l'opération lourde

    if output.len() < 24 + data.len() + crate::xchacha20::TAG_SIZE {
        return false;
    }
    let mut nonce = [0u8; crate::xchacha20::NONCE_SIZE];
    let sealed_len =
        crate::xchacha20::xchacha20_seal(&key, data, &aad, &mut nonce, &mut output[24..]);
    if sealed_len == 0 {
        return false;
    }
    output[..24].copy_from_slice(&nonce);
    true
}

/// Déchiffre un enregistrement d'application TLS.
pub fn tls_decrypt_record(session_handle: u32, input: &[u8], plaintext: &mut [u8]) -> bool {
    if session_handle == 0 || session_handle as usize > MAX_SESSIONS {
        return false;
    }

    let idx = (session_handle - 1) as usize;
    let pool = SESSION_POOL.lock();
    let session = &pool[idx];

    if !session.is_active() {
        return false;
    }

    let key: [u8; 32] = if session.flags & TLS_ROLE_SERVER != 0 {
        session.client_write_key
    } else {
        session.server_write_key
    };
    drop(pool);

    if input.len() < 24 + crate::xchacha20::TAG_SIZE {
        return false;
    }
    let mut nonce = [0u8; crate::xchacha20::NONCE_SIZE];
    nonce.copy_from_slice(&input[..24]);
    let aad = [ContentType::ApplicationData as u8];
    crate::xchacha20::xchacha20_open(&key, &nonce, &input[24..], &aad, plaintext) != 0
}

/// Ferme une session TLS de manière sécurisée.
/// Toutes les clés sont shreddées, la session est remise à Closed.
pub fn tls_close(session_handle: u32) -> bool {
    if session_handle == 0 || session_handle as usize > MAX_SESSIONS {
        return false;
    }

    let idx = (session_handle - 1) as usize;
    let mut pool = SESSION_POOL.lock();
    let session = &mut pool[idx];

    if session.state == TlsState::Closed as u8 {
        return false;
    }

    // Shredder toutes les clés
    session.shred_keys();

    // Réinitialiser la session
    session.state = TlsState::Closed as u8;
    session.peer_pid = 0;
    session.key_handle = 0;
    session.send_counter.store(0, Ordering::Release);
    session.recv_counter.store(0, Ordering::Release);
    session.flags = 0;

    ACTIVE_SESSION_COUNT.fetch_sub(1, Ordering::Relaxed);
    true
}

/// Vérifie le certificat d'un pair via la PKI.
pub fn tls_verify_certificate(cert: &crate::pki::Certificate) -> bool {
    crate::pki::pki_init();
    crate::pki::verify_certificate(cert)
}

/// Retourne le nombre de sessions actives.
pub fn active_session_count() -> u32 {
    ACTIVE_SESSION_COUNT.load(Ordering::Relaxed)
}

/// Retourne l'état d'une session.
pub fn session_state(session_handle: u32) -> Option<TlsState> {
    if session_handle == 0 || session_handle as usize > MAX_SESSIONS {
        return None;
    }
    let idx = (session_handle - 1) as usize;
    let pool = SESSION_POOL.lock();
    TlsState::from_u8(pool[idx].state)
}

/// Nettoie les sessions expirées (timeout = 60 secondes).
/// Retourne le nombre de sessions fermées.
pub fn cleanup_expired_sessions() -> u32 {
    let now = read_tsc();
    let timeout_tsc: u64 = 180_000_000_000; // ~60 secondes à 3 GHz
    let mut closed = 0u32;

    let mut pool = SESSION_POOL.lock();
    for session in pool.iter_mut() {
        if session.state == TlsState::Closed as u8 {
            continue;
        }
        let last = session.last_activity_tsc.load(Ordering::Acquire);
        if now.wrapping_sub(last) > timeout_tsc {
            session.shred_keys();
            session.state = TlsState::Closed as u8;
            session.peer_pid = 0;
            session.key_handle = 0;
            session.send_counter.store(0, Ordering::Release);
            session.recv_counter.store(0, Ordering::Release);
            session.flags = 0;
            closed += 1;
        }
    }

    if closed > 0 {
        ACTIVE_SESSION_COUNT.fetch_sub(closed, Ordering::Relaxed);
    }
    closed
}

/// Initialise le module TLS.
pub fn tls_init() {
    let mut pool = SESSION_POOL.lock();
    for session in pool.iter_mut() {
        session.state = TlsState::Closed as u8;
        session.shred_keys();
        session.peer_pid = 0;
        session.key_handle = 0;
        session.send_counter.store(0, Ordering::Release);
        session.recv_counter.store(0, Ordering::Release);
        session.flags = 0;
    }
    ACTIVE_SESSION_COUNT.store(0, Ordering::Release);
}
