#![allow(unused_imports, dead_code)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use exo_shield::{engine, ipc_gate};
use exo_syscall_abi as syscall;

// ── Message Types ───────────────────────────────────────────────────────────

pub const SCAN_REQUEST: u32 = 0;
pub const EVENT_REPORT: u32 = 1;
pub const QUARANTINE_CMD: u32 = 2;
pub const THREAT_QUERY: u32 = 3;
pub const POLICY_UPDATE: u32 = 4;
pub const HEARTBEAT: u32 = 5;
pub const PMC_ANOMALY_REPORT: u32 = 6;

// ── Reply Status Codes ──────────────────────────────────────────────────────

pub const SHIELD_OK: u32 = 0;
pub const SHIELD_ERR_ARGS: u32 = 1;
pub const SHIELD_ERR_BUSY: u32 = 2;
pub const SHIELD_ERR_NOT_FOUND: u32 = 3;
pub const SHIELD_ERR_DENIED: u32 = 4;
pub const SHIELD_ERR_CAP: u32 = 5;
pub const SHIELD_ERR_NOT_CONTAINED: u32 = 7;

pub const POLICY_SIGNATURE_PATTERN_OFFSET: usize = 8;
pub const POLICY_SIGNATURE_PATTERN_LEN_OFFSET: usize =
    POLICY_SIGNATURE_PATTERN_OFFSET + engine::SIGNATURE_PATTERN_SIZE;
pub const POLICY_SIGNATURE_NAME_LEN_OFFSET: usize = POLICY_SIGNATURE_PATTERN_LEN_OFFSET + 1;
pub const POLICY_SIGNATURE_NAME_OFFSET: usize = POLICY_SIGNATURE_NAME_LEN_OFFSET + 1;
const _: () = assert!(
    POLICY_SIGNATURE_NAME_OFFSET + engine::MAX_SIG_NAME <= syscall::IPC_INLINE_PAYLOAD_SIZE,
    "POLICY_UPDATE signature payload must fit in one IPC envelope"
);

// ── IPC Message Structures ──────────────────────────────────────────────────

/// Incoming IPC request (128 bytes).
///
/// When `ipc_gate` classifies a request as privileged, `payload[100..120]`
/// carries an `ExoCapTokenWire` targeting the live exo_shield service PID.
#[repr(C)]
pub struct ShieldRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

const _: () = assert!(core::mem::size_of::<ShieldRequest>() == syscall::IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(ShieldRequest, payload) == syscall::IPC_HEADER_SIZE);

/// Outgoing IPC reply (64 bytes).
#[repr(C)]
pub struct ShieldReply {
    pub status: u32,
    pub data: [u8; 56],
}

impl ShieldReply {
    pub fn new(status: u32) -> Self {
        ShieldReply {
            status: status,
            data: [0u8; 56],
        }
    }
}

// ── IPC Constants ───────────────────────────────────────────────────────────

pub const IPC_RECV_TIMEOUT_MS: u64 = 5_000;
pub const IPC_FLAG_TIMEOUT: u64 = syscall::IPC_FLAG_TIMEOUT;
pub const ETIMEDOUT: i64 = syscall::ETIMEDOUT;
pub const EXO_SHIELD_ENDPOINT: u64 = 10;
// FIX-SHIELD-PID (exoos_ipc_incoherences.md §3) : l'identité de politique IPC
// d'exo_shield est 10 (= EXO_SHIELD_ENDPOINT). L'ancienne valeur 12 entrait en
// collision avec TTY_SERVER_ENDPOINT et, après la correction de policy.rs,
// faisait tomber toutes les requêtes dans le défaut Deny (aucune règle dst=12).
// La valeur est importée de ipc_gate::policy pour interdire toute divergence.
pub const EXO_SHIELD_POLICY_ID: u32 = ipc_gate::policy::EXO_SHIELD_PID;
const _: () = assert!(EXO_SHIELD_POLICY_ID as u64 == EXO_SHIELD_ENDPOINT);
const _: () = assert!(syscall::EXO_CAP_TOKEN_WIRE_SIZE == ipc_gate::EXO_SHIELD_CAP_TOKEN_LEN);

/// PID runtime réel d'exo_shield, lu via SYS_GETPID au boot.
/// Requis par exo_cap_check : le kernel exige caller_pid == target_pid, donc la
/// cible du contrôle de capability doit être notre PID effectif (pas un PID
/// statique de convention qui peut diverger de l'ordre de spawn réel).
pub static RUNTIME_PID: AtomicU32 = AtomicU32::new(0);

#[inline]
pub fn runtime_pid() -> u32 {
    RUNTIME_PID.load(Ordering::Relaxed)
}

// ── Global Statistics ───────────────────────────────────────────────────────

pub static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static REQUESTS_OK: AtomicU64 = AtomicU64::new(0);
pub static REQUESTS_ERR: AtomicU64 = AtomicU64::new(0);
pub static IPC_RECV_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub static PERIODIC_SCAN_TICKS: AtomicU64 = AtomicU64::new(0);
pub static MAINTENANCE_TICKS: AtomicU64 = AtomicU64::new(0);

// ── Global Tick Counter ─────────────────────────────────────────────────────

pub static GLOBAL_TICK: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn boot_log(bytes: &[u8]) {
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        );
    }
}

pub fn current_tick() -> u64 {
    GLOBAL_TICK.load(Ordering::Relaxed)
}

pub fn advance_tick() {
    GLOBAL_TICK.fetch_add(1, Ordering::Relaxed);
}

#[inline(always)]
pub fn read_tsc() -> u64 {
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

// ── Payload Parsing Helpers ─────────────────────────────────────────────────

pub fn read_u32_le(payload: &[u8], offset: usize) -> u32 {
    if offset + 4 > payload.len() {
        return 0;
    }
    u32::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ])
}

pub fn read_u64_le(payload: &[u8], offset: usize) -> u64 {
    if offset + 8 > payload.len() {
        return 0;
    }
    u64::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
        payload[offset + 4],
        payload[offset + 5],
        payload[offset + 6],
        payload[offset + 7],
    ])
}

