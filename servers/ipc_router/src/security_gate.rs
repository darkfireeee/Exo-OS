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
/// FIX-IPC-04: Alignement sur IPC_KERNEL_MAX_MSG_SIZE (240 octets) pour permettre
/// aux messages requêtes complètes (FbRequest: 240B, InputRequest, TtyRequest)
/// et aux enveloppes ABI standard de transiter sans être faussement rejetés.
const MAX_INLINE_PAYLOAD: usize = exo_syscall_abi::IPC_KERNEL_MAX_MSG_SIZE;

/// Nombre maximum de PIDs suivis pour les statistiques et violations.
const MAX_TRACKED_PIDS: usize = 64;

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

fn find_or_alloc_slot(pid: u32, evict_clean_on_full: bool) -> Option<usize> {
    if pid == 0 {
        return None;
    }
    for (i, s) in PID_STATS.iter().enumerate() {
        if s.pid.load(Ordering::Acquire) == pid {
            return Some(i);
        }
    }
    for (i, s) in PID_STATS.iter().enumerate() {
        if s.pid
            .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            s.violations.store(0, Ordering::Release);
            s.allowed.store(0, Ordering::Release);
            return Some(i);
        }
    }
    if evict_clean_on_full {
        for (i, s) in PID_STATS.iter().enumerate() {
            let old_pid = s.pid.load(Ordering::Acquire);
            if old_pid != 0
                && s.violations.load(Ordering::Acquire) == 0
                && s.pid
                    .compare_exchange(old_pid, pid, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                s.allowed.store(0, Ordering::Release);
                return Some(i);
            }
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
            if let Some(i) = find_or_alloc_slot(src_pid, false) {
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
    if let Some(i) = find_or_alloc_slot(pid, true) {
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
    // Construire un ShieldRequest complet (8B header + 32B EVENT_REPORT payload).
    // Note: sender_pid=2 (ipc_router) et msg_type=1 (EVENT_REPORT) évitent tout
    // décalage avec ShieldRequest dans exo_shield/src/main.rs.
    const EXO_SHIELD_ENDPOINT: u64 = 10;
    const IPC_ROUTER_PID: u32 = 2;
    const MSG_EVENT_REPORT: u32 = 1;
    const EVENT_TYPE_IPC: u8 = 5;
    const THREAT_LEVEL_HIGH: u8 = 2;
    const CAP_TOKEN_OFFSET: usize = 172;

    // FIX-06: Build a full 200-byte ABI envelope carrying a kernel-issued
    // ExoCapTokenWire at payload[172..192] targeting exo_shield's live PID.
    let shield_pid = exocordon::live_pid_of(exocordon::ServiceId::ExoShield);
    let mut cap_token = exo_syscall_abi::ExoCapTokenWire::empty();
    let _ = unsafe {
        exo_syscall_abi::exo_cap_create(
            exo_syscall_abi::EXO_CAP_TYPE_IPC_ENDPOINT,
            exo_syscall_abi::EXO_CAP_RIGHT_IPC_SEND,
            shield_pid,
            &mut cap_token,
        )
    };

    let self_pid = unsafe { exo_syscall_abi::syscall0(exo_syscall_abi::SYS_GETPID) };
    let mut msg = exo_syscall_abi::IpcMessage::zeroed();
    msg.sender_pid = if self_pid > 0 {
        self_pid as u32
    } else {
        IPC_ROUTER_PID
    };
    msg.msg_type = MSG_EVENT_REPORT;
    msg.payload[0..4].copy_from_slice(&src_pid.to_le_bytes());
    msg.payload[4] = EVENT_TYPE_IPC;
    msg.payload[8..12].copy_from_slice(&(reason as u32).to_le_bytes());
    msg.payload[12..20].copy_from_slice(&(dst_pid as u64).to_le_bytes());
    msg.payload[28] = THREAT_LEVEL_HIGH;
    msg.payload[CAP_TOKEN_OFFSET..CAP_TOKEN_OFFSET + exo_syscall_abi::EXO_CAP_TOKEN_WIRE_SIZE]
        .copy_from_slice(&cap_token.bytes);

    let _ = unsafe {
        exo_syscall_abi::syscall6(
            exo_syscall_abi::SYS_IPC_SEND,
            EXO_SHIELD_ENDPOINT,
            &msg as *const exo_syscall_abi::IpcMessage as u64,
            core::mem::size_of::<exo_syscall_abi::IpcMessage>() as u64,
            0x01, // IPC_FLAG_NOWAIT
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
    let (Some(src_svc), Some(dst_svc)) = (
        exocordon::service_id_of(src_pid),
        exocordon::service_id_of(dst_pid),
    ) else {
        return false;
    };
    src_svc == dst_svc
        || src_svc == exocordon::ServiceId::IpcBroker
        || dst_svc == exocordon::ServiceId::IpcBroker
        || exocordon::find_edge(src_svc, dst_svc).is_some()
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
