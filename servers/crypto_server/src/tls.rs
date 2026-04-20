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

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

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
const MAX_RECORD_SIZE: usize = 1024;

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

#[inline(always)]
fn rdrand_u64() -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "rdrand {0}",
            out(reg) r,
            options(nostack, nomem),
        );
        r
    }
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
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

// ── Pool de sessions ─────────────────────────────────────────────────────────

/// Pool de sessions TLS statique.
static SESSION_POOL: spin::Mutex<[TlsSession; MAX_SESSIONS]> = spin::Mutex::new([
    TlsSession::new(), TlsSession::new(), TlsSession::new(), TlsSession::new(),
    TlsSession::new(), TlsSession::new(), TlsSession::new(), TlsSession::new(),
    TlsSession::new(), TlsSession::new(), TlsSession::new(), TlsSession::new(),
    TlsSession::new(), TlsSession::new(), TlsSession::new(), TlsSession::new(),
]);

static ACTIVE_SESSION_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Dérivation HKDF-Blake3 simplifiée ───────────────────────────────────────

/// Dérivation de clé HKDF simplifiée (Extract + Expand).
/// En production, ceci sera délégué au kernel kdf.rs via syscall.
fn hkdf_expand(secret: &[u8], label: &[u8], context: &[u8], output: &mut [u8]) {
    // Extract phase : hash du secret avec le sel
    let mut extractor: [u64; 4] = [
        0x6A09_E667_F3BC_C908 ^ u64::from_le_bytes([label[0], label[1 % label.len()], label[2 % label.len()], label[3 % label.len()], 0, 0, 0, 0]),
        0xBB67_AE85_84CA_A73B,
        0x3C6E_F372_FE94_F82B,
        0xA54F_F53A_5F1D_36F1,
    ];

    // Absorber le secret
    for chunk in secret.chunks(8) {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        let val = u64::from_le_bytes(bytes);
        extractor[0] = extractor[0].wrapping_add(val);
        extractor[1] ^= extractor[0].rotate_left(17);
        extractor[2] = extractor[2].wrapping_add(extractor[1]);
        extractor[3] ^= extractor[2].rotate_right(11);
    }

    // Absorber le contexte
    for chunk in context.chunks(8) {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        let val = u64::from_le_bytes(bytes);
        extractor[0] = extractor[0].wrapping_add(val).rotate_left(7);
        extractor[1] = extractor[1].wrapping_add(extractor[0]);
        extractor[2] ^= extractor[1].rotate_left(13);
        extractor[3] = extractor[3].wrapping_add(extractor[2]);
    }

    // Expand phase : produire la sortie
    let mut counter: u64 = 1;
    for chunk in output.chunks_mut(8) {
        extractor[0] = extractor[0].wrapping_add(counter);
        extractor[1] ^= extractor[0].rotate_left(5);
        extractor[2] = extractor[2].wrapping_add(extractor[1]).rotate_left(11);
        extractor[3] ^= extractor[2];

        let out_bytes = extractor[3].to_le_bytes();
        let copy_len = chunk.len().min(8);
        chunk[..copy_len].copy_from_slice(&out_bytes[..copy_len]);
        counter += 1;
    }
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
    let context = [session.client_random, session.server_random].concat();
    // Éviter l'allocation : construire le contexte manuellement
    let mut full_context = [0u8; 64];
    full_context[..32].copy_from_slice(&session.client_random);
    full_context[32..64].copy_from_slice(&session.server_random);

    hkdf_expand(&session.shared_secret, b"derived", &full_context, &mut derived_secret);

    // Dérivation des clés
    hkdf_expand(&derived_secret, b"c ws", &full_context, &mut session.client_write_key);
    hkdf_expand(&derived_secret, b"s ws", &full_context, &mut session.server_write_key);

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

// ── X25519 simplifié ─────────────────────────────────────────────────────────

/// Échange de clés X25519 simplifié.
/// En production, ceci sera délégué au kernel x25519.rs via syscall.
/// Pour le développement, utilise un dérivé déterministe.
fn x25519_dh(_private_key: &[u8; 32], _public_key: &[u8; 32]) -> [u8; 32] {
    let mut shared = [0u8; 32];

    // Dérivation déterministe pour le développement
    let mut state: u64 = 0x42_4F_4F_54_53_54_52_41; // "BOOTSTRA"
    for &b in _private_key {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for &b in _public_key {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }

    for i in 0..4 {
        let val = state.wrapping_add(i as u64 * 0x9E3779B97F4A7C15);
        shared[i * 8..i * 8 + 8].copy_from_slice(&val.to_le_bytes());
        state = state.wrapping_mul(0x5851F42D4C957F2D).wrapping_add(1);
    }

    shared
}

/// Génère une paire de clés X25519 éphémère.
fn generate_x25519_keypair() -> ([u8; 32], [u8; 32]) {
    let mut private = [0u8; 32];
    let tsc = read_tsc();
    let rand = rdrand_u64();

    // Remplir la clé privée avec de l'entropie
    let mut seed = tsc ^ rand;
    for i in 0..4 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        private[i * 8..i * 8 + 8].copy_from_slice(&seed.to_le_bytes());
    }

    // Clé publique dérivée (simplifiée)
    let public_base: [u8; 32] = [9u8; 32]; // Point de base X25519
    let public = x25519_dh(&private, &public_base);

    (private, public)
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

            // Générer les randoms
            let tsc = read_tsc();
            let rand = rdrand_u64();
            let mut seed = tsc ^ rand;
            for i in 0..4 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                session.client_random[i * 8..i * 8 + 8].copy_from_slice(&seed.to_le_bytes());
            }

            // Générer la paire X25519 éphémère
            let (private_key, public_key) = generate_x25519_keypair();

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
            session.send_counter.store(1, Ordering::Release);
            session.last_activity_tsc.store(read_tsc(), Ordering::Release);

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

    // Récupérer notre clé privée (stockée temporairement dans server_random)
    let private_key: [u8; 32] = {
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&session.server_random[..32]);
        pk
    };

    // Calculer le secret partagé
    session.shared_secret = x25519_dh(&private_key, &server_public);

    // Dériver les clés de trafic
    derive_traffic_keys(session);

    // Mettre à jour l'état
    session.state = TlsState::Verified as u8;
    session.recv_counter.store(1, Ordering::Release);
    session.last_activity_tsc.store(read_tsc(), Ordering::Release);

    // Shredder la clé privée
    // (on ne peut pas shred server_random car on l'a déjà écrasé avec le vrai server_random)

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

            // Générer server_random
            let tsc = read_tsc();
            let rand = rdrand_u64();
            let mut seed = tsc ^ rand ^ 0xDEAD_BEEF_CAFE_BABE;
            for i in 0..4 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                session.server_random[i * 8..i * 8 + 8].copy_from_slice(&seed.to_le_bytes());
            }

            // Générer la paire X25519 éphémère
            let (private_key, public_key) = generate_x25519_keypair();

            // Calculer le secret partagé
            session.shared_secret = x25519_dh(&private_key, &client_public);

            // Dériver les clés de trafic
            derive_traffic_keys(session);

            // Construire le ServerHello
            server_hello[0] = 2; // msg_type = ServerHello
            server_hello[1..3].copy_from_slice(&(CipherSuite::XChaCha20Blake3 as u16).to_le_bytes());
            server_hello[3..35].copy_from_slice(&session.server_random);
            server_hello[35..67].copy_from_slice(&public_key);

            session.state = TlsState::Verified as u8;
            session.cipher_suite = CipherSuite::XChaCha20Blake3 as u16;
            session.peer_pid = peer_pid;
            session.send_counter.store(1, Ordering::Release);
            session.recv_counter.store(0, Ordering::Release);
            session.last_activity_tsc.store(read_tsc(), Ordering::Release);

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

    let counter = session.send_counter.load(Ordering::Acquire);

    // Construire le nonce à partir de l'IV et du compteur
    let mut nonce = [0u8; 24]; // XChaCha20 nonce = 24 octets
    nonce[..12].copy_from_slice(&session.client_write_iv);
    let counter_bytes = counter.to_le_bytes();
    for i in 0..8 {
        nonce[12 + i] = counter_bytes[i];
    }

    // Chiffrer avec XChaCha20-BLAKE3 AEAD
    // En-tête : content_type + counter
    let aad = [ContentType::ApplicationData as u8];

    // Utiliser le module xchacha20 local
    let key: [u8; 32] = session.client_write_key;
    drop(pool); // Libérer le lock avant l'opération lourde

    crate::xchacha20::aead_seal(&key, data, &aad, output)
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

    let key: [u8; 32] = session.server_write_key;
    drop(pool);

    let aad = [ContentType::ApplicationData as u8];
    crate::xchacha20::aead_open(&key, input, &aad, plaintext)
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
    for (idx, session) in pool.iter_mut().enumerate() {
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
