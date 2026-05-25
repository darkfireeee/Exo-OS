//! # Signature Update System — Mise à jour, vérification Ed25519, rollback
//!
//! Système de mise à jour des signatures avec :
//! - Suivi de version
//! - Vérification Ed25519 déléguée au `crypto_server`
//! - Rollback (pile de 8 snapshots)
//! - Planification via TSC
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use exo_syscall_abi as syscall;
use spin::Mutex;

use super::database;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Profondeur maximale de rollback.
const MAX_ROLLBACK_DEPTH: usize = 8;

/// Taille d'une clé publique Ed25519 (32 octets).
pub const ED25519_PUBLIC_KEY_SIZE: usize = 32;

/// Taille d'une signature Ed25519 (64 octets).
pub const ED25519_SIGNATURE_SIZE: usize = 64;

/// Taille d'un bloc de mise à jour.
const MAX_UPDATE_PAYLOAD: usize = 4096;
const CRYPTO_SERVER_ENDPOINT: u64 = 4;
const CRYPTO_SERVER_PID: u32 = 5;
const EXO_SHIELD_CRYPTO_REPLY_SLOT: u64 = 0x5349_4755;
const CRYPTO_PROTOCOL_VERSION: u8 = 3;
const CRYPTO_REQUEST_PAYLOAD_SIZE: usize = 200;
const CRYPTO_REPLY_DATA_SIZE: usize = 224;
const VERIFY_UPDATE_CHUNK_SIZE: usize = CRYPTO_REQUEST_PAYLOAD_SIZE - 7;
const CRYPTO_VERIFY: u32 = 6;
const VERIFY_OP_BEGIN: u8 = 0;
const VERIFY_OP_UPDATE: u8 = 1;
const VERIFY_OP_FINAL: u8 = 2;
const CRYPTO_OK: u32 = 0;
const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct CryptoRequest {
    sender_pid: u32,
    msg_type: u32,
    reply_endpoint: u64,
    payload_len: u16,
    version: u8,
    flags: u8,
    cap_token: syscall::ExoCapTokenWire,
    payload: [u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
}

const _: () = assert!(core::mem::size_of::<CryptoRequest>() <= syscall::IPC_KERNEL_MAX_MSG_SIZE);

#[repr(C)]
#[derive(Clone, Copy)]
struct CryptoReply {
    sender_pid: u32,
    status: u32,
    key_handle: u32,
    data_len: u16,
    version: u8,
    flags: u8,
    data: [u8; CRYPTO_REPLY_DATA_SIZE],
}

impl CryptoReply {
    const fn empty() -> Self {
        Self {
            sender_pid: 0,
            status: 0,
            key_handle: 0,
            data_len: 0,
            version: 0,
            flags: 0,
            data: [0u8; CRYPTO_REPLY_DATA_SIZE],
        }
    }
}

static CRYPTO_VERIFY_IPC_LOCK: Mutex<()> = Mutex::new(());
static CRYPTO_SERVICE_TOKEN: Mutex<syscall::ExoCapTokenWire> =
    Mutex::new(syscall::ExoCapTokenWire::empty());

fn crypto_reply_endpoint() -> Option<u64> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return None;
    }
    Some(((pid as u64) << 32) | EXO_SHIELD_CRYPTO_REPLY_SLOT)
}

fn register_crypto_reply_endpoint(reply_endpoint: u64) -> bool {
    let name = b"exo_shield_sigupd";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            reply_endpoint,
        )
    };
    rc >= 0 || rc == syscall::EEXIST
}

fn crypto_ipc_roundtrip(req: &CryptoRequest, reply_endpoint: u64) -> Option<CryptoReply> {
    let send_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            CRYPTO_SERVER_ENDPOINT,
            req as *const CryptoRequest as u64,
            core::mem::size_of::<CryptoRequest>() as u64,
            syscall::IPC_FLAG_INJECT_SRC_PID,
            0,
            0,
        )
    };
    if send_rc < 0 {
        return None;
    }

    let mut reply = CryptoReply::empty();
    let recv_rc = unsafe {
        syscall::syscall4(
            syscall::SYS_EXO_IPC_RECV,
            reply_endpoint,
            &mut reply as *mut CryptoReply as u64,
            core::mem::size_of::<CryptoReply>() as u64,
            IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    };
    if recv_rc as usize != core::mem::size_of::<CryptoReply>() {
        return None;
    }
    if reply.version != CRYPTO_PROTOCOL_VERSION {
        return None;
    }
    Some(reply)
}

fn ensure_crypto_service_token() -> bool {
    let mut token = CRYPTO_SERVICE_TOKEN.lock();
    if !token.is_empty() {
        return true;
    }

    let rc = unsafe {
        syscall::exo_cap_create(
            syscall::EXO_CAP_TYPE_IPC_ENDPOINT,
            syscall::EXO_CAP_RIGHT_IPC_SEND,
            CRYPTO_SERVER_PID,
            &mut *token,
        )
    };
    rc >= 0 && !token.is_empty()
}

fn crypto_verify_finalize(ctx_handle: u32, reply_endpoint: u64) -> Option<CryptoReply> {
    let mut req = CryptoRequest {
        sender_pid: 0,
        msg_type: CRYPTO_VERIFY,
        reply_endpoint,
        payload_len: 5,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token: *CRYPTO_SERVICE_TOKEN.lock(),
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };
    req.payload[0] = VERIFY_OP_FINAL;
    req.payload[1..5].copy_from_slice(&ctx_handle.to_le_bytes());
    crypto_ipc_roundtrip(&req, reply_endpoint)
}

fn crypto_verify_ed25519(
    public_key: &[u8; ED25519_PUBLIC_KEY_SIZE],
    message: &[u8],
    signature: &[u8; ED25519_SIGNATURE_SIZE],
) -> bool {
    if message.is_empty() || message.len() > u16::MAX as usize || !ensure_crypto_service_token() {
        return false;
    }

    let Some(reply_endpoint) = crypto_reply_endpoint() else {
        return false;
    };
    if !register_crypto_reply_endpoint(reply_endpoint) {
        return false;
    }

    let _guard = CRYPTO_VERIFY_IPC_LOCK.lock();
    let cap_token = *CRYPTO_SERVICE_TOKEN.lock();

    let mut begin = CryptoRequest {
        sender_pid: 0,
        msg_type: CRYPTO_VERIFY,
        reply_endpoint,
        payload_len: 99,
        version: CRYPTO_PROTOCOL_VERSION,
        flags: 0,
        cap_token,
        payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
    };
    begin.payload[0] = VERIFY_OP_BEGIN;
    begin.payload[1..33].copy_from_slice(public_key);
    begin.payload[33..97].copy_from_slice(signature);
    begin.payload[97..99].copy_from_slice(&(message.len() as u16).to_le_bytes());

    let Some(begin_reply) = crypto_ipc_roundtrip(&begin, reply_endpoint) else {
        return false;
    };
    if begin_reply.status != CRYPTO_OK || begin_reply.key_handle == 0 {
        return false;
    }

    let ctx_handle = begin_reply.key_handle;
    for chunk in message.chunks(VERIFY_UPDATE_CHUNK_SIZE) {
        let mut update = CryptoRequest {
            sender_pid: 0,
            msg_type: CRYPTO_VERIFY,
            reply_endpoint,
            payload_len: (7 + chunk.len()) as u16,
            version: CRYPTO_PROTOCOL_VERSION,
            flags: 0,
            cap_token,
            payload: [0u8; CRYPTO_REQUEST_PAYLOAD_SIZE],
        };
        update.payload[0] = VERIFY_OP_UPDATE;
        update.payload[1..5].copy_from_slice(&ctx_handle.to_le_bytes());
        update.payload[5..7].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
        update.payload[7..7 + chunk.len()].copy_from_slice(chunk);

        let Some(update_reply) = crypto_ipc_roundtrip(&update, reply_endpoint) else {
            let _ = crypto_verify_finalize(ctx_handle, reply_endpoint);
            return false;
        };
        if update_reply.status != CRYPTO_OK || update_reply.key_handle != ctx_handle {
            let _ = crypto_verify_finalize(ctx_handle, reply_endpoint);
            return false;
        }
    }

    matches!(
        crypto_verify_finalize(ctx_handle, reply_endpoint),
        Some(reply) if reply.status == CRYPTO_OK
    )
}

// ═══════════════════════════════════════════════════════════════════════════════
// VÉRIFICATION ED25519
// ═══════════════════════════════════════════════════════════════════════════════

/// Vérifie une signature Ed25519 via le crypto_server centralisé.
///
/// SRV-02 interdit les primitives cryptographiques locales dans exo_shield;
/// cette API publique reste disponible mais délègue toujours au serveur crypto.
pub fn verify_ed25519(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    crypto_verify_ed25519(public_key, message, signature)
}
// ═══════════════════════════════════════════════════════════════════════════════
// VERSION ET MISE À JOUR
// ═══════════════════════════════════════════════════════════════════════════════

/// Version de la base de signatures.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
    pub build: u16,
}

impl UpdateVersion {
    pub const fn new(major: u16, minor: u16, patch: u16, build: u16) -> Self {
        Self {
            major,
            minor,
            patch,
            build,
        }
    }

    pub const fn zero() -> Self {
        Self {
            major: 0,
            minor: 0,
            patch: 0,
            build: 0,
        }
    }

    /// Encode en u64 pour comparaison.
    pub fn to_u64(&self) -> u64 {
        ((self.major as u64) << 48)
            | ((self.minor as u64) << 32)
            | ((self.patch as u64) << 16)
            | (self.build as u64)
    }

    pub fn is_newer_than(&self, other: &UpdateVersion) -> bool {
        self.to_u64() > other.to_u64()
    }
}

/// Statut de mise à jour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum UpdateStatus {
    Idle = 0,
    Downloading = 1,
    Verifying = 2,
    Applying = 3,
    Applied = 4,
    Failed = 5,
    RolledBack = 6,
}

// ── Snapshot pour rollback ───────────────────────────────────────────────────

/// Snapshot de la base de signatures pour rollback.
#[derive(Clone, Copy)]
struct RollbackSnapshot {
    entries: [database::SignatureEntry; database::MAX_SIGNATURES],
    count: usize,
    version: UpdateVersion,
    timestamp_tsc: u64,
    valid: bool,
}

impl RollbackSnapshot {
    const fn empty() -> Self {
        Self {
            entries: [database::SignatureEntry::empty(); database::MAX_SIGNATURES],
            count: 0,
            version: UpdateVersion::zero(),
            timestamp_tsc: 0,
            valid: false,
        }
    }

    fn reset(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = database::SignatureEntry::empty();
        }
        self.count = 0;
        self.version = UpdateVersion::zero();
        self.timestamp_tsc = 0;
        self.valid = false;
    }
}

// ── Mise à jour signée ───────────────────────────────────────────────────────

/// En-tête de mise à jour de signatures.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignatureUpdateHeader {
    /// Version de la mise à jour.
    pub version: UpdateVersion,
    /// Nombre de signatures dans cette mise à jour.
    pub signature_count: u32,
    /// Taille totale du payload (en octets).
    pub payload_size: u32,
    /// Clé publique Ed25519 de l'éditeur (32 octets).
    pub publisher_key: [u8; ED25519_PUBLIC_KEY_SIZE],
    /// Signature Ed25519 du payload (64 octets).
    pub signature: [u8; ED25519_SIGNATURE_SIZE],
    /// Horodatage TSC de la création.
    pub created_tsc: u64,
    /// Checksum CRC32 du payload.
    pub checksum: u32,
    /// Réservé.
    _reserved: [u8; 4],
}

impl SignatureUpdateHeader {
    pub const fn empty() -> Self {
        Self {
            version: UpdateVersion::zero(),
            signature_count: 0,
            payload_size: 0,
            publisher_key: [0u8; ED25519_PUBLIC_KEY_SIZE],
            signature: [0u8; ED25519_SIGNATURE_SIZE],
            created_tsc: 0,
            checksum: 0,
            _reserved: [0; 4],
        }
    }
}

const SIGNATURE_UPDATE_SIGNATURE_OFFSET: usize = core::mem::size_of::<UpdateVersion>()
    + core::mem::size_of::<u32>()
    + core::mem::size_of::<u32>()
    + ED25519_PUBLIC_KEY_SIZE;

const _: () = assert!(
    SIGNATURE_UPDATE_SIGNATURE_OFFSET
        + ED25519_SIGNATURE_SIZE
        + core::mem::size_of::<u64>()
        + core::mem::size_of::<u32>()
        + 4
        == core::mem::size_of::<SignatureUpdateHeader>(),
    "SignatureUpdateHeader layout inattendu"
);

// ── Entrée de signature encodée pour mise à jour ─────────────────────────────

/// Signature encodée dans un payload de mise à jour.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EncodedSignature {
    pub id: u32,
    pub pattern: [u8; database::PATTERN_SIZE],
    pub pattern_len: u8,
    pub severity: u8,
    pub category: u8,
    pub enabled: u8,
}

impl EncodedSignature {
    pub fn to_entry(&self) -> database::SignatureEntry {
        let mut entry = database::SignatureEntry::empty();
        entry.id = self.id;
        entry.pattern = self.pattern;
        entry.pattern_len = self.pattern_len;
        entry.severity =
            database::Severity::from_u8(self.severity).unwrap_or(database::Severity::Low);
        entry.category =
            database::Category::from_u8(self.category).unwrap_or(database::Category::Custom);
        entry.enabled = self.enabled != 0;
        entry
    }

    pub fn from_entry(entry: &database::SignatureEntry) -> Self {
        Self {
            id: entry.id,
            pattern: entry.pattern,
            pattern_len: entry.pattern_len,
            severity: entry.severity.as_u8(),
            category: entry.category.as_u8(),
            enabled: if entry.enabled { 1 } else { 0 },
        }
    }
}

// ── Calcul CRC32 ─────────────────────────────────────────────────────────────

/// Table CRC32 (IEEE 802.3).
static CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xedb88320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Calcule le CRC32 d'un buffer.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffffffffu32;
    for &byte in data.iter() {
        let idx = ((crc ^ byte as u32) & 0xff) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xffffffff
}

// ── Gestionnaire de mise à jour ──────────────────────────────────────────────

static UPDATE_MANAGER: Mutex<UpdateManagerInner> = Mutex::new(UpdateManagerInner::new());

static CURRENT_VERSION: AtomicU64 = AtomicU64::new(0);
static UPDATE_STATUS: AtomicU8 = AtomicU8::new(UpdateStatus::Idle as u8);
static LAST_UPDATE_TSC: AtomicU64 = AtomicU64::new(0);
static NEXT_SCHEDULED_TSC: AtomicU64 = AtomicU64::new(0);
static TOTAL_UPDATES_APPLIED: AtomicU32 = AtomicU32::new(0);
static TOTAL_UPDATES_FAILED: AtomicU32 = AtomicU32::new(0);

struct UpdateManagerInner {
    current_version: UpdateVersion,
    rollback_stack: [RollbackSnapshot; MAX_ROLLBACK_DEPTH],
    rollback_depth: usize,
    trusted_keys: [[u8; ED25519_PUBLIC_KEY_SIZE]; 4],
    trusted_key_count: usize,
}

impl UpdateManagerInner {
    const fn new() -> Self {
        Self {
            current_version: UpdateVersion::zero(),
            rollback_stack: [
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
                RollbackSnapshot::empty(),
            ],
            rollback_depth: 0,
            trusted_keys: [[0u8; ED25519_PUBLIC_KEY_SIZE]; 4],
            trusted_key_count: 0,
        }
    }
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Ajoute une clé publique de confiance pour la vérification des mises à jour.
pub fn add_trusted_key(key: &[u8; ED25519_PUBLIC_KEY_SIZE]) -> bool {
    let mut mgr = UPDATE_MANAGER.lock();
    if mgr.trusted_key_count >= 4 {
        return false;
    }

    // Vérifier si la clé existe déjà
    for i in 0..mgr.trusted_key_count {
        let mut diff = 0u8;
        for j in 0..ED25519_PUBLIC_KEY_SIZE {
            diff |= mgr.trusted_keys[i][j] ^ key[j];
        }
        if diff == 0 {
            return true; // Déjà présente
        }
    }

    let idx = mgr.trusted_key_count;
    mgr.trusted_keys[idx] = *key;
    mgr.trusted_key_count += 1;
    true
}

/// Retire une clé publique de confiance.
pub fn remove_trusted_key(key: &[u8; ED25519_PUBLIC_KEY_SIZE]) -> bool {
    let mut mgr = UPDATE_MANAGER.lock();
    for i in 0..mgr.trusted_key_count {
        let mut diff = 0u8;
        for j in 0..ED25519_PUBLIC_KEY_SIZE {
            diff |= mgr.trusted_keys[i][j] ^ key[j];
        }
        if diff == 0 {
            // Décaler les clés suivantes
            for k in i..mgr.trusted_key_count - 1 {
                mgr.trusted_keys[k] = mgr.trusted_keys[k + 1];
            }
            mgr.trusted_key_count -= 1;
            let last_idx = mgr.trusted_key_count;
            mgr.trusted_keys[last_idx] = [0u8; ED25519_PUBLIC_KEY_SIZE];
            return true;
        }
    }
    false
}

/// Vérifie la signature Ed25519 d'une mise à jour avec les clés de confiance.
fn verify_update_signature(header: &SignatureUpdateHeader, payload: &[u8]) -> bool {
    let mut trusted_keys = [[0u8; ED25519_PUBLIC_KEY_SIZE]; 4];
    let trusted_key_count = {
        let mgr = UPDATE_MANAGER.lock();
        for i in 0..mgr.trusted_key_count {
            trusted_keys[i] = mgr.trusted_keys[i];
        }
        mgr.trusted_key_count
    };

    let mut publisher_trusted = false;
    for trusted_key in trusted_keys.iter().take(trusted_key_count) {
        let mut diff = 0u8;
        for i in 0..ED25519_PUBLIC_KEY_SIZE {
            diff |= trusted_key[i] ^ header.publisher_key[i];
        }
        if diff == 0 {
            publisher_trusted = true;
            break;
        }
    }
    if !publisher_trusted {
        return false;
    }

    // Construire le message à vérifier : header (sans signature) || payload
    let header_size = core::mem::size_of::<SignatureUpdateHeader>();
    let msg_len = header_size + payload.len();
    if msg_len > MAX_UPDATE_PAYLOAD {
        return false;
    }

    let mut msg = [0u8; MAX_UPDATE_PAYLOAD];
    // Copier le header en mettant la signature à zéro
    let header_bytes = unsafe {
        core::slice::from_raw_parts(
            header as *const SignatureUpdateHeader as *const u8,
            header_size,
        )
    };
    msg[..header_size].copy_from_slice(header_bytes);
    msg[SIGNATURE_UPDATE_SIGNATURE_OFFSET
        ..SIGNATURE_UPDATE_SIGNATURE_OFFSET + ED25519_SIGNATURE_SIZE]
        .fill(0);
    if !payload.is_empty() {
        msg[header_size..header_size + payload.len()].copy_from_slice(payload);
    }

    crypto_verify_ed25519(&header.publisher_key, &msg[..msg_len], &header.signature)
}

/// Vérifie le CRC32 du payload.
fn verify_update_checksum(header: &SignatureUpdateHeader, payload: &[u8]) -> bool {
    let computed = crc32(payload);
    computed == header.checksum
}

/// Crée un snapshot de la base actuelle pour rollback.
fn create_snapshot(version: UpdateVersion) -> bool {
    let mut mgr = UPDATE_MANAGER.lock();

    if mgr.rollback_depth >= MAX_ROLLBACK_DEPTH {
        // Décaler la pile (FIFO : supprimer le plus ancien).
        mgr.rollback_stack.copy_within(1..MAX_ROLLBACK_DEPTH, 0);
        mgr.rollback_stack[MAX_ROLLBACK_DEPTH - 1].reset();
        mgr.rollback_depth = MAX_ROLLBACK_DEPTH - 1;
    }

    let idx = mgr.rollback_depth;
    mgr.rollback_stack[idx].version = version;
    mgr.rollback_stack[idx].timestamp_tsc = read_tsc();
    mgr.rollback_stack[idx].count = database::snapshot(
        &mut mgr.rollback_stack[idx].entries,
        database::MAX_SIGNATURES,
    );
    mgr.rollback_stack[idx].valid = true;
    mgr.rollback_depth += 1;

    true
}

/// Applique une mise à jour de signatures.
///
/// # Arguments
/// - `header` : en-tête de la mise à jour (avec signature).
/// - `payload` : données de la mise à jour (séquence d'EncodedSignature).
/// - `merge` : si true, fusionne avec les signatures existantes ; si false, remplace.
///
/// # Retour
/// - UpdateStatus::Applied si succès.
/// - UpdateStatus::Failed si échec.
pub fn apply_update(header: &SignatureUpdateHeader, payload: &[u8], merge: bool) -> UpdateStatus {
    UPDATE_STATUS.store(UpdateStatus::Verifying as u8, Ordering::Release);

    // 1. Vérifier que la version est plus récente
    let mgr = UPDATE_MANAGER.lock();
    let current_ver = mgr.current_version;
    drop(mgr);

    if !header.version.is_newer_than(&current_ver) && !merge {
        UPDATE_STATUS.store(UpdateStatus::Failed as u8, Ordering::Release);
        TOTAL_UPDATES_FAILED.fetch_add(1, Ordering::Relaxed);
        return UpdateStatus::Failed;
    }

    // 2. Vérifier le CRC32
    if !verify_update_checksum(header, payload) {
        UPDATE_STATUS.store(UpdateStatus::Failed as u8, Ordering::Release);
        TOTAL_UPDATES_FAILED.fetch_add(1, Ordering::Relaxed);
        return UpdateStatus::Failed;
    }

    // 3. Vérifier la signature Ed25519
    if !verify_update_signature(header, payload) {
        UPDATE_STATUS.store(UpdateStatus::Failed as u8, Ordering::Release);
        TOTAL_UPDATES_FAILED.fetch_add(1, Ordering::Relaxed);
        return UpdateStatus::Failed;
    }

    // 4. Créer un snapshot pour rollback
    if !create_snapshot(current_ver) {
        UPDATE_STATUS.store(UpdateStatus::Failed as u8, Ordering::Release);
        TOTAL_UPDATES_FAILED.fetch_add(1, Ordering::Relaxed);
        return UpdateStatus::Failed;
    }

    UPDATE_STATUS.store(UpdateStatus::Applying as u8, Ordering::Release);

    // 5. Décoder et appliquer les signatures
    let sig_size = core::mem::size_of::<EncodedSignature>();
    let expected_size = header.signature_count as usize * sig_size;

    if payload.len() < expected_size {
        // Rollback immédiat
        let _ = rollback();
        UPDATE_STATUS.store(UpdateStatus::Failed as u8, Ordering::Release);
        TOTAL_UPDATES_FAILED.fetch_add(1, Ordering::Relaxed);
        return UpdateStatus::Failed;
    }

    if !merge {
        // Remplacement complet : vider la base puis ajouter
        database::database_init();
    }

    let mut applied = 0u32;
    for i in 0..header.signature_count as usize {
        let offset = i * sig_size;
        if offset + sig_size > payload.len() {
            break;
        }

        // Décoder l'EncodedSignature
        let encoded: EncodedSignature =
            unsafe { core::ptr::read(payload[offset..].as_ptr() as *const EncodedSignature) };

        let entry = encoded.to_entry();
        if !entry.is_valid() {
            continue;
        }

        let added_id = database::add_signature_with_id(
            entry.id,
            &entry.pattern[..entry.pattern_len as usize],
            entry.severity,
            entry.category,
            entry.enabled,
        );

        if added_id != 0 {
            applied += 1;
        }
    }

    if header.signature_count > 0 && applied == 0 {
        let _ = rollback();
        UPDATE_STATUS.store(UpdateStatus::Failed as u8, Ordering::Release);
        TOTAL_UPDATES_FAILED.fetch_add(1, Ordering::Relaxed);
        return UpdateStatus::Failed;
    }

    // 6. Mettre à jour la version
    let mut mgr = UPDATE_MANAGER.lock();
    mgr.current_version = header.version;
    drop(mgr);

    CURRENT_VERSION.store(header.version.to_u64(), Ordering::Release);
    LAST_UPDATE_TSC.store(read_tsc(), Ordering::Release);
    TOTAL_UPDATES_APPLIED.fetch_add(1, Ordering::Relaxed);

    UPDATE_STATUS.store(UpdateStatus::Applied as u8, Ordering::Release);
    UpdateStatus::Applied
}

/// Effectue un rollback vers la version précédente.
///
/// # Retour
/// - true si le rollback a réussi, false si pas de snapshot disponible.
pub fn rollback() -> bool {
    let mut mgr = UPDATE_MANAGER.lock();

    if mgr.rollback_depth == 0 {
        return false;
    }

    mgr.rollback_depth -= 1;
    let snapshot_idx = mgr.rollback_depth;
    let snapshot = &mgr.rollback_stack[snapshot_idx];
    if !snapshot.valid {
        mgr.rollback_depth += 1;
        return false;
    }

    // Restaurer les signatures
    let restored = database::restore(&snapshot.entries[..snapshot.count]);
    let snapshot_version = snapshot.version;
    mgr.current_version = snapshot_version;

    // Invalider le snapshot
    mgr.rollback_stack[snapshot_idx].valid = false;

    CURRENT_VERSION.store(snapshot_version.to_u64(), Ordering::Release);
    LAST_UPDATE_TSC.store(read_tsc(), Ordering::Release);
    UPDATE_STATUS.store(UpdateStatus::RolledBack as u8, Ordering::Release);

    let _ = restored;
    true
}

/// Planifie une vérification de mise à jour à un TSC futur.
pub fn schedule_update_check(tsc_deadline: u64) {
    NEXT_SCHEDULED_TSC.store(tsc_deadline, Ordering::Release);
}

/// Vérifie si une mise à jour est due (à appeler périodiquement).
///
/// # Retour
/// - true si une vérification est due, false sinon.
pub fn is_update_due() -> bool {
    let deadline = NEXT_SCHEDULED_TSC.load(Ordering::Acquire);
    if deadline == 0 {
        return false;
    }
    let now = read_tsc();
    now >= deadline
}

/// Retourne la version actuelle.
pub fn get_current_version() -> UpdateVersion {
    let mgr = UPDATE_MANAGER.lock();
    mgr.current_version
}

/// Retourne le statut actuel.
pub fn get_update_status() -> UpdateStatus {
    match UPDATE_STATUS.load(Ordering::Acquire) {
        0 => UpdateStatus::Idle,
        1 => UpdateStatus::Downloading,
        2 => UpdateStatus::Verifying,
        3 => UpdateStatus::Applying,
        4 => UpdateStatus::Applied,
        5 => UpdateStatus::Failed,
        6 => UpdateStatus::RolledBack,
        _ => UpdateStatus::Idle,
    }
}

/// Statistiques de mise à jour.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UpdateStats {
    pub current_version: UpdateVersion,
    pub status: UpdateStatus,
    pub updates_applied: u32,
    pub updates_failed: u32,
    pub rollback_depth: usize,
    pub last_update_tsc: u64,
    pub next_scheduled_tsc: u64,
}

/// Retourne les statistiques de mise à jour.
pub fn get_update_stats() -> UpdateStats {
    let mgr = UPDATE_MANAGER.lock();
    UpdateStats {
        current_version: mgr.current_version,
        status: get_update_status(),
        updates_applied: TOTAL_UPDATES_APPLIED.load(Ordering::Relaxed),
        updates_failed: TOTAL_UPDATES_FAILED.load(Ordering::Relaxed),
        rollback_depth: mgr.rollback_depth,
        last_update_tsc: LAST_UPDATE_TSC.load(Ordering::Relaxed),
        next_scheduled_tsc: NEXT_SCHEDULED_TSC.load(Ordering::Relaxed),
    }
}

/// Initialise le gestionnaire de mise à jour.
pub fn update_init() {
    if let Some(reply_endpoint) = crypto_reply_endpoint() {
        let _ = register_crypto_reply_endpoint(reply_endpoint);
    }
    let _ = ensure_crypto_service_token();
    let mut mgr = UPDATE_MANAGER.lock();
    mgr.current_version = UpdateVersion::new(1, 0, 0, 0);
    mgr.rollback_depth = 0;
    mgr.trusted_key_count = 0;
    for i in 0..MAX_ROLLBACK_DEPTH {
        mgr.rollback_stack[i].reset();
    }
    for i in 0..4 {
        mgr.trusted_keys[i] = [0u8; ED25519_PUBLIC_KEY_SIZE];
    }

    CURRENT_VERSION.store(mgr.current_version.to_u64(), Ordering::Release);
    UPDATE_STATUS.store(UpdateStatus::Idle as u8, Ordering::Release);
    LAST_UPDATE_TSC.store(0, Ordering::Release);
    NEXT_SCHEDULED_TSC.store(0, Ordering::Release);
    TOTAL_UPDATES_APPLIED.store(0, Ordering::Release);
    TOTAL_UPDATES_FAILED.store(0, Ordering::Release);
}
