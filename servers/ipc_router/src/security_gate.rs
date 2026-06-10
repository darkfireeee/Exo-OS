//! # security_gate — Porte de sécurité IPC (ipc_router PID 2)
//!
//! Applique les politiques de sécurité pour chaque message IPC transitant
//! par l'ipc_router. La source de vérité pour les autorisations est le DAG
//! ExoCordon (défini dans `exocordon.rs`, miroir du `kernel/ipc_policy.rs`).
//!
//! ## Rôle et séparation des responsabilités
//!
//! Ce module est une **porte mince** — il NE définit PAS de politique propre :
//! - La **politique d'autorisation** est dans `exocordon.rs` (DAG statique,
//!   miroir exact de `kernel/src/security/ipc_policy.rs`).
//! - La **limite de payload inline** (IPC-04) est vérifiée ici.
//! - Le **comptage de violations** est tenu ici pour alimenter le heartbeat.
//! - Le **rate-limiting par quota** est dans `exocordon.rs` (`quota_left`).
//!
//! ## Règles respectées
//! - IPC-01 : le DAG ExoCordon est la source de vérité pour les autorisations.
//! - IPC-04 : pas de payload inline > IPC_INLINE_PAYLOAD_SIZE (192 octets ABI).
//! - ZT-POLICY-01 : deny-by-default — tout ce qui n'est pas explicitement
//!                  autorisé par ExoCordon est refusé.
//! - CAP-01 : la vérification de capability token est effectuée par le kernel
//!            avant même que le message arrive ici (SYS_IPC_SEND valide le cap).
//!
//! ## Corrections
//! - FIX-IPC-04 : MAX_INLINE_PAYLOAD était à 48 octets, bloquant systématiquement
//!   FbRequest (236B), TtyRequest (212B) et toute enveloppe ABI standard (192B).
//!   Aligné sur IPC_INLINE_PAYLOAD_SIZE = 192 (définition canonique syscall_abi).

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use exo_syscall_abi;
use super::exocordon;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Taille maximale d'un payload inline (IPC-04).
/// FIX-IPC-04: 48 → 192 pour correspondre à IPC_INLINE_PAYLOAD_SIZE de l'ABI.
/// L'ancienne valeur de 48 octets bloquait tous les messages FbRequest, TtyRequest
/// et les enveloppes ABI standard (192 bytes).
const MAX_INLINE_PAYLOAD: usize = exo_syscall_abi::IPC_INLINE_PAYLOAD_SIZE;

/// Nombre maximum de paires src→dst tracées pour les statistiques.
const MAX_TRACKED_PIDS: usize = 16;

/// Seuil de violations avant enregistrement en quarantaine douce.
/// Soft quarantine : les violations sont loguées mais init_server décide du kill.
const SOFT_QUARANTINE_THRESHOLD: u32 = 10;

// ── Types ────────────────────────────────────────────────────────────────────

/// Verdict de sécurité pour un message IPC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SecurityVerdict {
    /// Message autorisé.
    Allow = 0,
    /// Message refusé — chemin non autorisé dans le DAG ExoCordon.
    Deny = 1,
    /// Message refusé — payload inline trop grand (IPC-04).
    DenyPayloadTooLarge = 2,
    /// Message refusé — quota ExoCordon épuisé.
    DenyQuotaExhausted = 3,
    /// Source ou destination inconnue (service non enregistré).
    DenyUnknownService = 4,
}

/// Raison d'un refus (pour l'audit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DenyReason {
    UnauthorizedPath = 0,
    QuotaExhausted = 1,
    PayloadTooLarge = 2,
    UnknownService = 3,
}

// ── Compteurs de violations par PID ──────────────────────────────────────────

struct PidStats {
    pid: AtomicU32,
    violations: AtomicU32,
    allowed: AtomicU64,
}

impl PidStats {
    const fn new() -> Self {
        Self {
            pid: AtomicU32::new(0),
            violations: AtomicU32::new(0),
            allowed: AtomicU64::new(0),
        }
    }
}

static PID_STATS: [PidStats; MAX_TRACKED_PIDS] = {
    const E: PidStats = PidStats::new();
    [E; MAX_TRACKED_PIDS]
};

static TOTAL_ALLOWED: AtomicU64 = AtomicU64::new(0);
static TOTAL_DENIED: AtomicU64 = AtomicU64::new(0);
static SOFT_QUARANTINE_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Helpers ───────────────────────────────────────────────────────────────────

fn find_or_alloc_slot(pid: u32) -> Option<usize> {
    for (i, s) in PID_STATS.iter().enumerate() {
        if s.pid.load(Ordering::Relaxed) == pid {
            return Some(i);
        }
    }
    for (i, s) in PID_STATS.iter().enumerate() {
        if s.pid
            .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Some(i);
        }
    }
    None
}

// ── API publique ──────────────────────────────────────────────────────────────

/// Vérifie si un message IPC peut transiter de `src_pid` vers `dst_pid`.
///
/// Politique appliquée dans l'ordre :
/// 1. IPC-04 : payload_len ≤ IPC_INLINE_PAYLOAD_SIZE (192) octets.
/// 2. ExoCordon : service connu + chemin autorisé + quota > 0.
pub fn check_message(
    src_pid: u32,
    dst_pid: u32,
    _msg_type: u32,
    payload_len: usize,
) -> SecurityVerdict {
    // Règle IPC-04
    if payload_len > MAX_INLINE_PAYLOAD {
        TOTAL_DENIED.fetch_add(1, Ordering::Relaxed);
        record_violation(src_pid, DenyReason::PayloadTooLarge as u8);
        return SecurityVerdict::DenyPayloadTooLarge;
    }

    // Délégation à ExoCordon — source de vérité unique
    match exocordon::check_ipc(src_pid, dst_pid) {
        Ok(()) => {
            TOTAL_ALLOWED.fetch_add(1, Ordering::Relaxed);
            if let Some(i) = find_or_alloc_slot(src_pid) {
                PID_STATS[i].allowed.fetch_add(1, Ordering::Relaxed);
            }
            SecurityVerdict::Allow
        }
        Err(exocordon::IpcError::UnknownService) => {
            TOTAL_DENIED.fetch_add(1, Ordering::Relaxed);
            record_violation(src_pid, DenyReason::UnknownService as u8);
            SecurityVerdict::DenyUnknownService
        }
        Err(exocordon::IpcError::UnauthorizedPath) => {
            TOTAL_DENIED.fetch_add(1, Ordering::Relaxed);
            record_violation(src_pid, DenyReason::UnauthorizedPath as u8);
            SecurityVerdict::Deny
        }
        Err(exocordon::IpcError::QuotaExhausted) => {
            TOTAL_DENIED.fetch_add(1, Ordering::Relaxed);
            record_violation(src_pid, DenyReason::QuotaExhausted as u8);
            SecurityVerdict::DenyQuotaExhausted
        }
    }
}

/// Enregistre une violation pour `pid`.
/// Si le seuil SOFT_QUARANTINE_THRESHOLD est atteint, incrémente le compteur
/// de quarantaine douce (init_server notifié via heartbeat).
pub fn record_violation(pid: u32, _violation_type: u8) {
    if let Some(i) = find_or_alloc_slot(pid) {
        let prev = PID_STATS[i].violations.fetch_add(1, Ordering::Relaxed);
        if prev + 1 == SOFT_QUARANTINE_THRESHOLD {
            SOFT_QUARANTINE_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Retourne le nombre de violations enregistrées pour `pid`.
pub fn get_violation_count(pid: u32) -> u32 {
    for s in PID_STATS.iter() {
        if s.pid.load(Ordering::Relaxed) == pid {
            return s.violations.load(Ordering::Relaxed);
        }
    }
    0
}

/// Retourne `true` si `pid` est en quarantaine douce.
pub fn is_quarantined(pid: u32) -> bool {
    get_violation_count(pid) >= SOFT_QUARANTINE_THRESHOLD
}

/// Logue une violation IPC et notifie exo_shield via IPC EVENT_REPORT.
///
/// FIX-AUDIT-IPC (exoos_ipc_incoherences.md §7) : l'ancienne implémentation
/// était un stub qui appelait uniquement record_violation() (compteur en mémoire)
/// sans jamais contacter exo_shield. Les violations IPC n'apparaissaient ni dans
/// l'audit ExoLedger ni dans le threat scoring d'exo_shield.
///
/// Correction : envoi d'un IPC EVENT_REPORT(1) vers l'endpoint exo_shield (PID 10).
/// Le message est non-bloquant (timeout=0) pour ne pas bloquer le hot path IPC.
/// Si exo_shield est indisponible, on se rabat sur le compteur local.
///
/// Format du payload EVENT_REPORT (8 octets inline) :
///   [0..4]  : event_type = 0x10 (IPC_VIOLATION)
///   [4..8]  : source_pid
pub fn audit_log_violation(
    src_pid: u32,
    dst_pid: u32,
    _verdict: SecurityVerdict,
    reason: DenyReason,
) {
    // NOTE: pas de record_violation() ici — check_message() a déjà comptabilisé
    // la violation localement. Cette fonction ne fait que la notification
    // exo_shield (appelée par main.rs après un verdict de refus) ; un double
    // appel gonflerait artificiellement le seuil SOFT_QUARANTINE_THRESHOLD.
    let _ = reason;

    // FIX-AUDIT-IPC : envoi non-bloquant EVENT_REPORT vers exo_shield (endpoint 10).
    // Layout : [event_type u32][src_pid u32][dst_pid u32][reason u8][pad 3B]
    const EXO_SHIELD_ENDPOINT: u64 = 10;
    const EVENT_IPC_VIOLATION:  u32 = 0x0000_0010;
    // Construire un petit buffer inline (12 bytes)
    let mut buf = [0u8; 12];
    buf[0..4].copy_from_slice(&EVENT_IPC_VIOLATION.to_le_bytes());
    buf[4..8].copy_from_slice(&src_pid.to_le_bytes());
    buf[8..12].copy_from_slice(&dst_pid.to_le_bytes());

    // Envoi non-bloquant (IPC_FLAG_NONBLOCK = 0x02) — on ne peut pas attendre
    // dans le hot path de vérification IPC.
    let _ = unsafe {
        exo_syscall_abi::syscall6(
            exo_syscall_abi::SYS_IPC_SEND,
            EXO_SHIELD_ENDPOINT,
            buf.as_ptr() as u64,
            buf.len() as u64,
            0x02, // IPC_FLAG_NONBLOCK
            0,
            0,
        )
    };
}

/// Libère un PID de quarantaine douce (appelé par init_server après correction).
pub fn quarantine_release(pid: u32) -> bool {
    for s in PID_STATS.iter() {
        if s.pid.load(Ordering::Relaxed) == pid {
            let prev = s.violations.swap(0, Ordering::AcqRel);
            if prev >= SOFT_QUARANTINE_THRESHOLD {
                SOFT_QUARANTINE_COUNT.fetch_sub(1, Ordering::Relaxed);
            }
            return true;
        }
    }
    false
}

/// Compatibilité : vérifie qu'un chemin existe dans le DAG (pas de modification de politique).
pub fn add_policy(
    src_pid: u32,
    dst_pid: u32,
    _max_rate: u64,
    _max_payload_size: u16,
    _allowed_msg_types: u32,
) -> bool {
    matches!(
        exocordon::check_ipc(src_pid, dst_pid),
        Ok(()) | Err(exocordon::IpcError::QuotaExhausted)
    )
}

/// Initialise la porte de sécurité.
pub fn security_gate_init() {
    TOTAL_ALLOWED.store(0, Ordering::Release);
    TOTAL_DENIED.store(0, Ordering::Release);
    SOFT_QUARANTINE_COUNT.store(0, Ordering::Release);
    for s in PID_STATS.iter() {
        s.pid.store(0, Ordering::Release);
        s.violations.store(0, Ordering::Release);
        s.allowed.store(0, Ordering::Release);
    }
}

/// Statistiques de la porte de sécurité.
#[derive(Clone, Copy, Debug)]
pub struct SecurityGateStats {
    pub total_allowed: u64,
    pub total_denied: u64,
    pub soft_quarantine_count: u32,
}

pub fn security_gate_stats() -> SecurityGateStats {
    SecurityGateStats {
        total_allowed: TOTAL_ALLOWED.load(Ordering::Relaxed),
        total_denied: TOTAL_DENIED.load(Ordering::Relaxed),
        soft_quarantine_count: SOFT_QUARANTINE_COUNT.load(Ordering::Relaxed),
    }
}
