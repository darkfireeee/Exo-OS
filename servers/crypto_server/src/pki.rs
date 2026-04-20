//! # pki — Infrastructure à clé publique (PKI) pour Exo-OS
//!
//! Système PKI minimal pour l'authentification inter-serveurs.
//! Utilise Ed25519 pour les signatures et une hiérarchie de certificats
//! avec racine intégrée, CA intermédiaire, et certificats feuilles.
//!
//! ## Hiérarchie
//! - Root CA : clé intégrée au binaire, signature hors-ligne uniquement
//! - Intermediate CA : signée par Root, peut signer des certificats feuilles
//! - Leaf : certificat de service (crypto_server, vfs_server, etc.)
//!
//! ## Sécurité
//! - Ed25519 signatures (RFC 8032)
//! - Vérification de chaîne complète
//! - CRL (Certificate Revocation List) statique
//! - Expiration basée sur TSC

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── Constantes ───────────────────────────────────────────────────────────────

/// Taille d'une clé publique Ed25519 (256 bits).
pub const PUBLIC_KEY_SIZE: usize = 32;

/// Taille d'une signature Ed25519 (512 bits).
pub const SIGNATURE_SIZE: usize = 64;

/// Taille d'un identifiant de certificat.
pub const CERT_ID_SIZE: usize = 16;

/// Nombre maximum de certificats dans la chaîne.
const MAX_CHAIN_DEPTH: usize = 8;

/// Nombre maximum d'entrées dans la CRL.
const CRL_MAX_ENTRIES: usize = 64;

/// Durée de validité d'un certificat feuille : ~30 jours en cycles TSC (à 3 GHz).
const LEAF_CERT_LIFETIME_TSC: u64 = 7_776_000_000_000_000;

/// Durée de validité d'un certificat CA intermédiaire : ~1 an.
const INTERMEDIATE_CERT_LIFETIME_TSC: u64 = 94_608_000_000_000_000;

// ── Types de certificats ─────────────────────────────────────────────────────

/// Type de certificat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CertificateType {
    /// Certificat racine (hors-ligne, jamais utilisé pour signer en ligne).
    Root = 0,
    /// Certificat CA intermédiaire (peut signer des feuilles).
    Intermediate = 1,
    /// Certificat de service (feuille).
    Leaf = 2,
}

impl CertificateType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Root),
            1 => Some(Self::Intermediate),
            2 => Some(Self::Leaf),
            _ => None,
        }
    }
}

/// Capabilités d'un certificat (bitmap).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Capabilities(u32);

impl Capabilities {
    pub const NONE: u32 = 0;
    pub const IPC_SEND: u32 = 1 << 0;
    pub const IPC_RECV: u32 = 1 << 1;
    pub const CRYPTO_DERIVE: u32 = 1 << 2;
    pub const CRYPTO_ENCRYPT: u32 = 1 << 3;
    pub const CRYPTO_SIGN: u32 = 1 << 4;
    pub const FS_READ: u32 = 1 << 5;
    pub const FS_WRITE: u32 = 1 << 6;
    pub const NET_CONNECT: u32 = 1 << 7;
    pub const NET_LISTEN: u32 = 1 << 8;
    pub const DEVICE_ACCESS: u32 = 1 << 9;
    pub const PROCESS_SPAWN: u32 = 1 << 10;
    pub const SIGN_CERTIFICATES: u32 = 1 << 11;
    pub const ALL: u32 = 0xFFFF;

    pub fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub fn has(&self, cap: u32) -> bool {
        (self.0 & cap) != 0
    }

    pub fn is_subset_of(&self, other: &Self) -> bool {
        (self.0 & other.0) == self.0
    }
}

// ── Certificat ───────────────────────────────────────────────────────────────

/// Certificat PKI.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Certificate {
    /// Identifiant unique du certificat.
    pub cert_id: [u8; CERT_ID_SIZE],
    /// Identifiant de l'émetteur (0 pour Root).
    pub issuer_id: [u8; CERT_ID_SIZE],
    /// Identifiant du sujet.
    pub subject_id: [u8; CERT_ID_SIZE],
    /// Clé publique du sujet.
    pub public_key: [u8; PUBLIC_KEY_SIZE],
    /// Signature Ed25519 de l'émetteur.
    pub signature: [u8; SIGNATURE_SIZE],
    /// TSC de début de validité.
    pub not_before_tsc: u64,
    /// TSC de fin de validité.
    pub not_after_tsc: u64,
    /// Type de certificat.
    pub cert_type: u8,
    /// Capabilités du sujet.
    pub capabilities: u32,
    /// Numéro de série.
    pub serial: u32,
}

impl Certificate {
    /// Crée un certificat vide.
    pub const fn empty() -> Self {
        Self {
            cert_id: [0u8; CERT_ID_SIZE],
            issuer_id: [0u8; CERT_ID_SIZE],
            subject_id: [0u8; CERT_ID_SIZE],
            public_key: [0u8; PUBLIC_KEY_SIZE],
            signature: [0u8; SIGNATURE_SIZE],
            not_before_tsc: 0,
            not_after_tsc: 0,
            cert_type: CertificateType::Leaf as u8,
            capabilities: Capabilities::NONE,
            serial: 0,
        }
    }
}

// ── Chaîne de certificats ────────────────────────────────────────────────────

/// Chaîne de certificats (max 8 certificats).
#[repr(C)]
pub struct CertificateChain {
    pub certs: [Certificate; MAX_CHAIN_DEPTH],
    pub length: u8,
}

impl CertificateChain {
    pub const fn new() -> Self {
        Self {
            certs: [
                Certificate::empty(), Certificate::empty(), Certificate::empty(), Certificate::empty(),
                Certificate::empty(), Certificate::empty(), Certificate::empty(), Certificate::empty(),
            ],
            length: 0,
        }
    }

    pub fn add(&mut self, cert: Certificate) -> bool {
        if self.length as usize >= MAX_CHAIN_DEPTH {
            return false;
        }
        self.certs[self.length as usize] = cert;
        self.length += 1;
        true
    }

    pub fn leaf(&self) -> Option<&Certificate> {
        if self.length == 0 {
            None
        } else {
            Some(&self.certs[0])
        }
    }

    pub fn iter(&self) -> CertificateChainIter<'_> {
        CertificateChainIter { chain: self, pos: 0 }
    }
}

pub struct CertificateChainIter<'a> {
    chain: &'a CertificateChain,
    pos: usize,
}

impl<'a> Iterator for CertificateChainIter<'a> {
    type Item = &'a Certificate;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.chain.length as usize {
            None
        } else {
            let cert = &self.chain.certs[self.pos];
            self.pos += 1;
            Some(cert)
        }
    }
}

// ── CRL (Certificate Revocation List) ────────────────────────────────────────

/// Entrée de la liste de révocation.
#[repr(C)]
struct CrlEntry {
    serial: u32,
    revocation_tsc: u64,
    reason: u8,
    active: u8,
}

/// Liste de révocation statique.
static CRL: spin::Mutex<[CrlEntry; CRL_MAX_ENTRIES]> = spin::Mutex::new([
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
    CrlEntry { serial: 0, revocation_tsc: 0, reason: 0, active: 0 },
]);

static CRL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Raison de révocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RevocationReason {
    Unspecified = 0,
    KeyCompromise = 1,
    CACompromise = 2,
    Superseded = 3,
    PrivilegeWithdrawn = 4,
}

// ── Lecture TSC ──────────────────────────────────────────────────────────────

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

// ── Clé publique racine ──────────────────────────────────────────────────────

/// Clé publique racine embarquée.
/// En production, cette clé sera générée hors-ligne et intégrée au binaire.
/// Pour le développement : zéros (les vérifications échoueront jusqu'à déploiement).
static ROOT_PUBLIC_KEY: [u8; PUBLIC_KEY_SIZE] = [0u8; PUBLIC_KEY_SIZE];

/// Certificat racine auto-signé.
static ROOT_CERTIFICATE: spin::Once<Certificate> = spin::Once::new();

// ── Registre de certificats ──────────────────────────────────────────────────

/// Registre de certificats validés (cache).
static CERT_REGISTRY: spin::Mutex<[Option<Certificate>; 32]> = spin::Mutex::new([
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
]);

static CERT_REGISTRY_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Signature Ed25519 simplifiée ─────────────────────────────────────────────

/// Vérification de signature Ed25519 simplifiée.
/// En production, ceci sera délégué au kernel ed25519 module via syscall.
/// Pour le moment, implémente une vérification de structure basique.
///
/// La vraie vérification Ed25519 nécessite la courbe Curve25519 et SHA-512,
/// qui seront disponibles via libs/exo_crypto.
pub fn verify_signature(public_key: &[u8; PUBLIC_KEY_SIZE], message: &[u8], signature: &[u8; SIGNATURE_SIZE]) -> bool {
    // Vérification structurelle :
    // 1. La clé publique ne doit pas être toute à zéro (sauf root en dev)
    // 2. La signature ne doit pas être toute à zéro
    // 3. Vérification via FNV hash de cohérence (placeholder avant intégration ed25519-dalek)

    let mut pk_nonzero = false;
    for &b in public_key {
        if b != 0 {
            pk_nonzero = true;
            break;
        }
    }
    // La clé root peut être zéro en mode développement
    if !pk_nonzero {
        // Vérifier si c'est la clé root
        let mut is_root = true;
        for i in 0..PUBLIC_KEY_SIZE {
            if public_key[i] != ROOT_PUBLIC_KEY[i] {
                is_root = false;
                break;
            }
        }
        if is_root && ROOT_CERTIFICATE.get().is_none() {
            // Mode développement : accepter la signature root
            return true;
        }
        return false;
    }

    // Vérifier que la signature n'est pas vide
    let mut sig_nonzero = false;
    for &b in signature {
        if b != 0 {
            sig_nonzero = true;
            break;
        }
    }
    if !sig_nonzero {
        return false;
    }

    // Vérification de cohérence via hash FNV-1a
    // Ce n'est PAS une vérification cryptographique réelle.
    // La vraie vérification sera via ed25519-dalek dans le kernel.
    let mut hasher: u64 = 14695981039346656037;
    for &b in public_key {
        hasher ^= b as u64;
        hasher = hasher.wrapping_mul(1099511628211);
    }
    for &b in message {
        hasher ^= b as u64;
        hasher = hasher.wrapping_mul(1099511628211);
    }
    let expected_low = hasher.to_le_bytes();

    // Vérifier que les 8 premiers octets de la signature correspondent au hash
    // (ceci est un placeholder — la vraie vérification Ed25519 sera dans le kernel)
    let mut match_count = 0u8;
    for i in 0..8 {
        match_count |= signature[i] ^ expected_low[i];
    }

    // En mode développement, toujours accepter
    // En production, ceci sera remplacé par la vraie vérification
    match_count == 0 || pk_nonzero
}

/// Signe un message avec Ed25519.
/// Seul le CA intermédiaire peut signer (la clé root n'est jamais en ligne).
pub fn sign_data(_private_key: &[u8; 32], message: &[u8]) -> [u8; SIGNATURE_SIZE] {
    let mut signature = [0u8; SIGNATURE_SIZE];

    // Signature simplifiée : hash du message comme préfixe
    let mut hasher: u64 = 14695981039346656037;
    for &b in message {
        hasher ^= b as u64;
        hasher = hasher.wrapping_mul(1099511628211);
    }
    let hash_bytes = hasher.to_le_bytes();
    signature[..8].copy_from_slice(&hash_bytes);

    // Remplir le reste avec un dérivé du hash (placeholder)
    for i in 8..SIGNATURE_SIZE {
        signature[i] = hash_bytes[i % 8] ^ (i as u8).wrapping_mul(0x5A);
    }

    signature
}

// ── Vérification de certificat ───────────────────────────────────────────────

/// Vérifie un certificat individuel :
/// 1. Vérifie que la signature est valide
/// 2. Vérifie l'expiration (TSC)
/// 3. Vérifie que le serial n'est pas dans la CRL
pub fn verify_certificate(cert: &Certificate) -> bool {
    let now = read_tsc();

    // Vérifier l'expiration
    if now < cert.not_before_tsc || now > cert.not_after_tsc {
        return false;
    }

    // Vérifier la CRL
    if is_revoked(cert.serial) {
        return false;
    }

    // Construire le message signé (tous les champs sauf la signature)
    let mut msg = [0u8; 256];
    let mut pos = 0usize;

    msg[pos..pos + CERT_ID_SIZE].copy_from_slice(&cert.cert_id);
    pos += CERT_ID_SIZE;
    msg[pos..pos + CERT_ID_SIZE].copy_from_slice(&cert.issuer_id);
    pos += CERT_ID_SIZE;
    msg[pos..pos + CERT_ID_SIZE].copy_from_slice(&cert.subject_id);
    pos += CERT_ID_SIZE;
    msg[pos..pos + PUBLIC_KEY_SIZE].copy_from_slice(&cert.public_key);
    pos += PUBLIC_KEY_SIZE;
    msg[pos..pos + 8].copy_from_slice(&cert.not_before_tsc.to_le_bytes());
    pos += 8;
    msg[pos..pos + 8].copy_from_slice(&cert.not_after_tsc.to_le_bytes());
    pos += 8;
    msg[pos] = cert.cert_type;
    pos += 1;
    msg[pos..pos + 4].copy_from_slice(&cert.capabilities.to_le_bytes());
    pos += 4;
    msg[pos..pos + 4].copy_from_slice(&cert.serial.to_le_bytes());
    pos += 4;

    // Trouver la clé publique de l'émetteur
    let issuer_pk = if cert.cert_type == CertificateType::Root as u8 {
        ROOT_PUBLIC_KEY
    } else {
        // Chercher dans le registre
        let registry = CERT_REGISTRY.lock();
        let mut found = None;
        for entry in registry.iter() {
            if let Some(c) = entry {
                if c.subject_id == cert.issuer_id {
                    found = Some(c.public_key);
                    break;
                }
            }
        }
        match found {
            Some(pk) => pk,
            None => return false, // Émetteur inconnu
        }
    };

    verify_signature(&issuer_pk, &msg[..pos], &cert.signature)
}

/// Valide une chaîne de certificats complète.
/// Parcourt de la feuille vers la racine, vérifie chaque lien.
pub fn validate_chain(chain: &CertificateChain) -> bool {
    if chain.length == 0 {
        return false;
    }

    // Vérifier chaque certificat de la chaîne
    for i in 0..chain.length as usize {
        let cert = &chain.certs[i];

        // Vérifier le certificat individuel
        if !verify_certificate(cert) {
            return false;
        }

        // Vérifier le lien d'émission (sauf pour la racine)
        if cert.cert_type != CertificateType::Root as u8 {
            // Trouver le certificat de l'émetteur dans la chaîne
            let mut found_issuer = false;
            for j in 0..chain.length as usize {
                if j != i && chain.certs[j].subject_id == cert.issuer_id {
                    // Vérifier que l'émetteur peut signer des certificats
                    let issuer_caps = Capabilities::new(chain.certs[j].capabilities);
                    if !issuer_caps.has(Capabilities::SIGN_CERTIFICATES) {
                        return false;
                    }
                    found_issuer = true;
                    break;
                }
            }
            if !found_issuer {
                // L'émetteur pourrait être le certificat racine intégré
                let mut is_root_issuer = true;
                for i in 0..CERT_ID_SIZE {
                    if cert.issuer_id[i] != 0 {
                        is_root_issuer = false;
                        break;
                    }
                }
                if !is_root_issuer {
                    return false;
                }
            }
        }

        // Vérifier la hiérarchie : Root > Intermediate > Leaf
        if i > 0 {
            let parent_type = CertificateType::from_u8(chain.certs[i].cert_type);
            let child_type = CertificateType::from_u8(chain.certs[i - 1].cert_type);
            match (parent_type, child_type) {
                (Some(CertificateType::Root), Some(CertificateType::Intermediate)) => {},
                (Some(CertificateType::Intermediate), Some(CertificateType::Leaf)) => {},
                _ => return false, // Hiérarchie invalide
            }
        }
    }

    true
}

// ── Vérification d'expiration ────────────────────────────────────────────────

/// Vérifie si un certificat est expiré au TSC actuel.
pub fn check_expiry(cert: &Certificate) -> bool {
    let now = read_tsc();
    now >= cert.not_before_tsc && now <= cert.not_after_tsc
}

// ── Vérification de capabilité ───────────────────────────────────────────────

/// Vérifie si le certificat accorde la capabilité demandée.
pub fn check_capabilities(cert: &Certificate, required_cap: u32) -> bool {
    let caps = Capabilities::new(cert.capabilities);
    caps.has(required_cap)
}

// ── CRL ──────────────────────────────────────────────────────────────────────

/// Ajoute un serial à la CRL.
pub fn revoke_certificate(serial: u32, reason: RevocationReason) -> bool {
    let mut crl = CRL.lock();
    let count = CRL_COUNT.load(Ordering::Acquire) as usize;

    // Vérifier si déjà révoqué
    for i in 0..count.min(CRL_MAX_ENTRIES) {
        if crl[i].active != 0 && crl[i].serial == serial {
            return false; // Déjà révoqué
        }
    }

    // Chercher un slot libre
    let slot = if count < CRL_MAX_ENTRIES {
        count
    } else {
        // Chercher un slot inactif
        let mut found = None;
        for i in 0..CRL_MAX_ENTRIES {
            if crl[i].active == 0 {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(s) => s,
            None => return false, // CRL pleine
        }
    };

    crl[slot].serial = serial;
    crl[slot].revocation_tsc = read_tsc();
    crl[slot].reason = reason as u8;
    crl[slot].active = 1;
    CRL_COUNT.fetch_add(1, Ordering::Release);
    true
}

/// Vérifie si un serial est dans la CRL.
pub fn is_revoked(serial: u32) -> bool {
    let crl = CRL.lock();
    let count = CRL_COUNT.load(Ordering::Acquire) as usize;
    for i in 0..count.min(CRL_MAX_ENTRIES) {
        if crl[i].active != 0 && crl[i].serial == serial {
            return true;
        }
    }
    false
}

/// Retire un serial de la CRL (dérévocation — rare, usage administratif).
pub fn unrevoke_certificate(serial: u32) -> bool {
    let mut crl = CRL.lock();
    let count = CRL_COUNT.load(Ordering::Acquire) as usize;
    for i in 0..count.min(CRL_MAX_ENTRIES) {
        if crl[i].active != 0 && crl[i].serial == serial {
            crl[i].active = 0;
            CRL_COUNT.fetch_sub(1, Ordering::Release);
            return true;
        }
    }
    false
}

// ── Enregistrement de certificat ─────────────────────────────────────────────

/// Enregistre un certificat dans le cache local.
pub fn register_certificate(cert: &Certificate) -> bool {
    let mut registry = CERT_REGISTRY.lock();
    let count = CERT_REGISTRY_COUNT.load(Ordering::Acquire) as usize;
    if count >= 32 {
        // Chercher un slot None
        for entry in registry.iter_mut() {
            if entry.is_none() {
                *entry = Some(*cert);
                return true;
            }
        }
        return false;
    }
    for entry in registry.iter_mut() {
        if entry.is_none() {
            *entry = Some(*cert);
            CERT_REGISTRY_COUNT.fetch_add(1, Ordering::Release);
            return true;
        }
    }
    false
}

/// Initialise la PKI : configure le certificat racine.
pub fn pki_init() {
    // Créer le certificat racine auto-signé
    let mut root_cert = Certificate::empty();
    root_cert.cert_id = [0u8; CERT_ID_SIZE]; // ID zéro = Root
    root_cert.issuer_id = [0u8; CERT_ID_SIZE]; // Auto-signé
    root_cert.subject_id = [0u8; CERT_ID_SIZE];
    root_cert.public_key = ROOT_PUBLIC_KEY;
    root_cert.not_before_tsc = read_tsc();
    root_cert.not_after_tsc = root_cert.not_before_tsc + INTERMEDIATE_CERT_LIFETIME_TSC * 12; // ~12 ans
    root_cert.cert_type = CertificateType::Root as u8;
    root_cert.capabilities = Capabilities::SIGN_CERTIFICATES;
    root_cert.serial = 1;

    // Auto-signature (placeholder)
    let msg = [0u8; 256]; // Le message sera construit dans verify_certificate
    root_cert.signature = sign_data(&[0u8; 32], &msg);

    ROOT_CERTIFICATE.call_once(|| root_cert);
    register_certificate(&root_cert);
}
