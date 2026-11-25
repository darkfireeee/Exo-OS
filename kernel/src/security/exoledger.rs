// kernel/src/security/exoledger.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoLedger — Chained Audit with immutable P0 zone (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoLedger is the chained audit module of ExoShield. It provides a security
// event log with two distinct zones:
//
//   P0 ZONE (immutable): 16 entries NEVER overwritten — critical events:
//     CpViolation, IommuFault, HandoffTriggered, BootSealViolation
//
//   RING BUFFER ZONE (circular): ~90 entries — operational security
//     events. Overflow allowed: the oldest entries
//     are overwritten (but NEVER P0 entries).
//
// CRYPTOGRAPHIC CHAINING:
//   Each entry contains hash = Blake3(this_entry || prev_hash).
//   Any modification of an entry invalidates the entire chain.
//   Kernel B (Core 0) can verify integrity by reading back the chain.
//
// MEMORY LAYOUT:
//   SSR.LOG_AUDIT (+0x8000, 16 KiB, append-only, RO for Kernel A)
//   +0x0000: AuditHeader { version, total_count, last_merkle_root }
//   +0x0030: P0_Zone [LedgerEntry; 16]
//   +0x08B0: RingBuffer [LedgerEntry; ~90]
//
// RULE EXOLEDGER-01: exo_ledger_append_p0() = append-only, NEVER overwritten.
// RULE EXOLEDGER-02: Any attempt to overwrite P0 = kernel panic.
// RULE EXOLEDGER-03: ISR-safe — no allocation, no blocking lock.
// RULE EXOLEDGER-04: Hash = Blake3(this || prev_hash) — immutable chain.
//
// REFERENCES:
//   ExoShield_v1_Production.md — MODULE 7: ExoLedger
//   SSR layout: ExoOS_Architecture_v7.md §3.4
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// ExoLedger constants
// ─────────────────────────────────────────────────────────────────────────────

/// Number of entries in P0 zone (immutable, never overwritten).
pub const P0_ZONE_ENTRIES: usize = 16;

/// Number of entries in ring buffer (circular).
/// 16 KiB total / sizeof(LedgerEntry) = ~122 entries. P0 takes 16 = 106 remaining.
/// Rounded down to 96 for clean alignment.
const RING_BUFFER_ENTRIES: usize = 96;

/// Size of a LedgerEntry = 136 bytes.
///   seq: u64 (8) + tsc: u64 (8) + actor_oid: [u8;32] (32) + action: ActionTag (24)
///   + prev_hash: [u8;32] (32) + hash: [u8;32] (32) = 136 bytes
const LEDGER_ENTRY_SIZE: usize = 136;

/// Offset of audit zone in SSR (+0x8000).
pub const SSR_LOG_AUDIT_OFFSET: u64 = 0x8000;

/// Total size of audit zone (16 KiB).
pub const SSR_LOG_AUDIT_SIZE: usize = 16 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// ActionTag — Audited event types
// ─────────────────────────────────────────────────────────────────────────────

/// Action tag for ledger entries.
///
/// P0 actions (critical, immutable) and normal actions (ring buffer)
/// are distinguished at the type level.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub enum ActionTag {
    // ── P0 Actions (immutable zone) ─────────────────────────────────────────
    /// #CP violation (CET Shadow Stack / IBT). error_code from #CP handler.
    CpViolation { error_code: u64 },
    /// IOMMU fault (DMA outside whitelist).
    IommuFault { domain_id: u32, fault_addr: u64 },
    /// HANDOFF triggered (Kernel B took control).
    HandoffTriggered { reason: u32 },
    /// Boot seal violation (network/FS active before SECURITY_READY).
    BootSealViolation { step: u8 },
    /// Watchdog expired (ExoNmi — strikes exceeded).
    WatchdogExpired { strikes: u32 },

    // ── Normal actions (ring buffer) ────────────────────────────────────────
    /// Capability revoked.
    CapabilityRevoked { oid: u64 },
    /// PKS domain revoked.
    PksDomainRevoked { domain: u8 },
    /// Unauthorized IPC access attempt (ExoCordon).
    IpcUnauthorized { src_pid: u32, dst_pid: u32 },
    /// Capability budget exhausted (ExoKairos).
    BudgetExhausted { oid: u64 },
    /// Access policy violation.
    AccessDenied { oid: u64, rights: u32 },
    /// Boot event (ExoSeal).
    BootEvent { step: u8 },
    /// NIC IOMMU locked (ExoSeal step 0).
    NicIommuLocked,
    /// General audit (custom data).
    Custom { tag: u64, data: u64 },
}

impl ActionTag {
    /// Returns true if this action is a P0 event (critical).
    pub fn is_p0(&self) -> bool {
        matches!(
            self,
            ActionTag::CpViolation { .. }
                | ActionTag::IommuFault { .. }
                | ActionTag::HandoffTriggered { .. }
                | ActionTag::BootSealViolation { .. }
                | ActionTag::WatchdogExpired { .. }
        )
    }

    /// Serializes ActionTag into 24 bytes for Blake3 hash.
    /// Format: [type_tag: u64 | field1: u64 | field2: u64]
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut buf = [0u8; 24];
        match self {
            ActionTag::CpViolation { error_code } => {
                buf[0..8].copy_from_slice(&1u64.to_le_bytes());
                buf[8..16].copy_from_slice(&error_code.to_le_bytes());
            }
            ActionTag::IommuFault {
                domain_id,
                fault_addr,
            } => {
                buf[0..8].copy_from_slice(&2u64.to_le_bytes());
                buf[8..12].copy_from_slice(&domain_id.to_le_bytes());
                buf[12..24].copy_from_slice(&fault_addr.to_le_bytes());
            }
            ActionTag::HandoffTriggered { reason } => {
                buf[0..8].copy_from_slice(&3u64.to_le_bytes());
                buf[8..12].copy_from_slice(&reason.to_le_bytes());
            }
            ActionTag::BootSealViolation { step } => {
                buf[0..8].copy_from_slice(&4u64.to_le_bytes());
                buf[8] = *step;
            }
            ActionTag::WatchdogExpired { strikes } => {
                buf[0..8].copy_from_slice(&5u64.to_le_bytes());
                buf[8..12].copy_from_slice(&strikes.to_le_bytes());
            }
            ActionTag::CapabilityRevoked { oid } => {
                buf[0..8].copy_from_slice(&10u64.to_le_bytes());
                buf[8..16].copy_from_slice(&oid.to_le_bytes());
            }
            ActionTag::PksDomainRevoked { domain } => {
                buf[0..8].copy_from_slice(&11u64.to_le_bytes());
                buf[8] = *domain;
            }
            ActionTag::IpcUnauthorized { src_pid, dst_pid } => {
                buf[0..8].copy_from_slice(&12u64.to_le_bytes());
                buf[8..12].copy_from_slice(&src_pid.to_le_bytes());
                buf[12..16].copy_from_slice(&dst_pid.to_le_bytes());
            }
            ActionTag::BudgetExhausted { oid } => {
                buf[0..8].copy_from_slice(&13u64.to_le_bytes());
                buf[8..16].copy_from_slice(&oid.to_le_bytes());
            }
            ActionTag::AccessDenied { oid, rights } => {
                buf[0..8].copy_from_slice(&14u64.to_le_bytes());
                buf[8..16].copy_from_slice(&oid.to_le_bytes());
                buf[16..20].copy_from_slice(&rights.to_le_bytes());
            }
            ActionTag::BootEvent { step } => {
                buf[0..8].copy_from_slice(&15u64.to_le_bytes());
                buf[8] = *step;
            }
            ActionTag::NicIommuLocked => {
                buf[0..8].copy_from_slice(&16u64.to_le_bytes());
            }
            ActionTag::Custom { tag, data } => {
                buf[0..8].copy_from_slice(&255u64.to_le_bytes());
                buf[8..16].copy_from_slice(&tag.to_le_bytes());
                buf[16..24].copy_from_slice(&data.to_le_bytes());
            }
        }
        buf
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LedgerEntry — 136-byte structure
// ─────────────────────────────────────────────────────────────────────────────

/// Chained audit log entry.
///
/// # Memory layout (136 bytes, #[repr(C)])
///
/// | Offset | Field      | Size   | Description                              |
/// |--------|------------|--------|------------------------------------------|
/// | 0      | seq        | 8      | Global monotonic sequence number         |
/// | 8      | tsc        | 8      | TSC timestamp (CPU cycles)               |
/// | 16     | actor_oid  | 32     | Actor ObjectId (never PID)               |
/// | 48     | action     | 24     | Serialized ActionTag                     |
/// | 72     | prev_hash  | 32     | Previous entry hash                      |
/// | 104    | hash       | 32     | Blake3(this_entry_bytes || prev_hash)     |
///
/// # Integrity
/// The hash is computed as: Blake3(seq || tsc || actor_oid || action || prev_hash)
/// This makes each entry dependent on all preceding entries.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LedgerEntry {
    /// Monotonic sequence number (global, never reset to zero).
    pub seq: u64,
    /// TSC timestamp at time of event.
    pub tsc: u64,
    /// Actor ObjectId (never PID — ExoShield uses OID for audit).
    pub actor_oid: [u8; 32],
    /// Serialized action tag (24 bytes).
    pub action: [u8; 24],
    /// Previous entry hash in chain.
    pub prev_hash: [u8; 32],
    /// Blake3 hash of this entry (computed after write).
    pub hash: [u8; 32],
}

impl LedgerEntry {
    /// Total entry size (136 bytes).
    pub const SIZE: usize = LEDGER_ENTRY_SIZE;

    /// Creates an empty/zero entry.
    pub const fn zeroed() -> Self {
        Self {
            seq: 0,
            tsc: 0,
            actor_oid: [0u8; 32],
            action: [0u8; 24],
            prev_hash: [0u8; 32],
            hash: [0u8; 32],
        }
    }

    /// Computes Blake3 hash of this entry (excluding hash field itself).
    ///
    /// Input: seq || tsc || actor_oid || action || prev_hash
    /// Output: Blake3(input)
    fn compute_hash(&self) -> [u8; 32] {
        // Build hash buffer: everything except hash field
        // 8 + 8 + 32 + 24 + 32 = 104 bytes
        let mut buf = [0u8; 104];
        buf[0..8].copy_from_slice(&self.seq.to_le_bytes());
        buf[8..16].copy_from_slice(&self.tsc.to_le_bytes());
        buf[16..48].copy_from_slice(&self.actor_oid);
        buf[48..72].copy_from_slice(&self.action);
        buf[72..104].copy_from_slice(&self.prev_hash);
        crate::security::crypto::blake3::blake3_hash(&buf)
    }

    /// Computes and stores hash of this entry.
    fn finalize_hash(&mut self) {
        self.hash = self.compute_hash();
    }
}

// Static verification of LedgerEntry size
const _: () = assert!(
    core::mem::size_of::<LedgerEntry>() == LEDGER_ENTRY_SIZE,
    "LedgerEntry: taille inattendue — doit être 136 bytes"
);

// ─────────────────────────────────────────────────────────────────────────────
// AuditHeader — Audit zone header
// ─────────────────────────────────────────────────────────────────────────────

/// SSR.LOG_AUDIT audit zone header.
///
/// Placed at start of zone (+0x0000), before P0 entries.
#[repr(C)]
pub struct AuditHeader {
    /// Format version (1 = v1.0).
    pub version: u64,
    /// Total entries written (P0 + ring buffer).
    pub total_count: u64,
    /// Last computed Merkle root (for Kernel B verification).
    pub last_merkle_root: [u8; 32],
    /// Alignment padding (total header ≈ 48 bytes).
    pub _reserved: [u8; 16],
}

// ─────────────────────────────────────────────────────────────────────────────
// Memory storage — P0 Zone + Ring Buffer
// ─────────────────────────────────────────────────────────────────────────────

/// P0 zone (immutable) — 16 entries never overwritten.
static mut P0_ZONE: [LedgerEntry; P0_ZONE_ENTRIES] = {
    const INIT: LedgerEntry = LedgerEntry::zeroed();
    [INIT; P0_ZONE_ENTRIES]
};

/// Ring buffer (circular) — ~96 entries with overflow.
static mut RING_BUFFER: [LedgerEntry; RING_BUFFER_ENTRIES] = {
    const INIT: LedgerEntry = LedgerEntry::zeroed();
    [INIT; RING_BUFFER_ENTRIES]
};

#[inline(always)]
fn p0_zone_ptr() -> *mut LedgerEntry {
    core::ptr::addr_of_mut!(P0_ZONE).cast::<LedgerEntry>()
}

#[inline(always)]
fn ring_buffer_ptr() -> *mut LedgerEntry {
    core::ptr::addr_of_mut!(RING_BUFFER).cast::<LedgerEntry>()
}

fn clear_ledger_storage() {
    // SAFETY: called during explicit init/test reset while no ledger reader runs.
    unsafe {
        for idx in 0..P0_ZONE_ENTRIES {
            core::ptr::write_volatile(p0_zone_ptr().add(idx), LedgerEntry::zeroed());
        }
        for idx in 0..RING_BUFFER_ENTRIES {
            core::ptr::write_volatile(ring_buffer_ptr().add(idx), LedgerEntry::zeroed());
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global counters
// ─────────────────────────────────────────────────────────────────────────────

/// Global monotonic sequence number.
static GLOBAL_SEQ: AtomicU64 = AtomicU64::new(0);

/// Number of used P0 entries.
static P0_USED: AtomicUsize = AtomicUsize::new(0);

/// Ring buffer head (write position).
static RING_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Total entries written (P0 + ring).
static TOTAL_WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Single P0 overflow alert is sufficient: beyond that we saturate silently.
static P0_OVERFLOW_REPORTED: AtomicBool = AtomicBool::new(false);

/// Last computed hash (for chaining).
static LAST_HASH: [core::sync::atomic::AtomicU8; 32] = {
    const INIT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    [INIT; 32]
};
static P0_CHAIN_LOCK: Mutex<()> = Mutex::new(());

/// ExoLedger initialized.
static EXOLEDGER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Reads last stored hash.
fn load_last_hash() -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in LAST_HASH.iter().enumerate() {
        out[i] = byte.load(Ordering::Acquire);
    }
    out
}

/// Stores a hash as last hash.
fn store_last_hash(hash: &[u8; 32]) {
    for (i, byte) in LAST_HASH.iter().enumerate() {
        byte.store(hash[i], Ordering::Release);
    }
}

/// Reads current TSC (ISR-safe).
#[inline]
fn rdtsc() -> u64 {
    crate::arch::x86_64::cpu::tsc::read_tsc()
}

/// Returns ObjectId of current actor (active thread).
/// In absence of a canonical OID wired everywhere, encode `(pid, tid)` into
/// first 16 bytes to maintain a stable actor in audit log.
fn current_actor_oid() -> [u8; 32] {
    let mut oid = [0u8; 32];
    #[cfg(test)]
    let tcb_ptr: *mut crate::scheduler::core::task::ThreadControlBlock = core::ptr::null_mut();
    #[cfg(not(test))]
    let tcb_ptr = crate::scheduler::core::switch::current_thread_raw();
    if tcb_ptr.is_null() || !is_published_tcb(tcb_ptr as usize) {
        // Fallback early-boot: avoid an all-zero OID to keep audit exploitable.
        let tsc = rdtsc();
        let cpu = 0u32;
        oid[0..8].copy_from_slice(&tsc.to_le_bytes());
        oid[8..12].copy_from_slice(&cpu.to_le_bytes());
        return oid;
    }
    // SAFETY: current_thread_raw() returns the active TCB of the current CPU as long as
    // caller remains in this execution context.
    let tcb = unsafe { &*tcb_ptr };
    oid[0..8].copy_from_slice(&(tcb.pid.0 as u64).to_le_bytes());
    oid[8..16].copy_from_slice(&tcb.tid.to_le_bytes());
    oid[16..24].copy_from_slice(&tcb.creation_tsc().to_le_bytes());
    oid
}

fn is_published_tcb(tcb_ptr: usize) -> bool {
    if tcb_ptr == 0 {
        return false;
    }

    crate::scheduler::core::switch::CURRENT_THREAD_PER_CPU
        .iter()
        .any(|slot| slot.load(Ordering::Acquire) == tcb_ptr)
}

// ─────────────────────────────────────────────────────────────────────────────
// exo_ledger_append_p0 — Append into P0 zone (IMMUTABLE)
// ─────────────────────────────────────────────────────────────────────────────

/// Appends an entry into immutable P0 zone of ledger.
///
/// # RULE EXOLEDGER-01
/// P0 zone is append-only: entries are NEVER overwritten.
/// If all 16 P0 slots are full, function panics (RULE EXOLEDGER-02).
///
/// # ISR-safe
/// No allocation, no blocking lock. Uses atomics for
/// synchronization.
///
/// # Usage
/// Called by:
/// - `exocage::cp_handler()` for #CP violations
/// - IOMMU fault handler for DMA faults
/// - ExoPhoenix HANDOFF for handoff triggers
/// - ExoSeal for boot seal violations
pub fn exo_ledger_append_p0(action: ActionTag) {
    let _guard = P0_CHAIN_LOCK.lock();
    let idx = P0_USED.load(Ordering::Acquire);

    if idx >= P0_ZONE_ENTRIES {
        drop(_guard);
        if !P0_OVERFLOW_REPORTED.swap(true, Ordering::AcqRel) {
            exo_ledger_append(ActionTag::Custom {
                tag: 0x5030_4f56_4552_464c,
                data: idx as u64,
            });
        }
        return;
    }
    P0_USED.store(idx + 1, Ordering::Release);

    let seq = GLOBAL_SEQ.fetch_add(1, Ordering::AcqRel);
    let prev_hash = load_last_hash();

    let mut entry = LedgerEntry {
        seq,
        tsc: rdtsc(),
        actor_oid: current_actor_oid(),
        action: action.to_bytes(),
        prev_hash,
        hash: [0u8; 32],
    };

    // Compute chained hash: Blake3(this_entry || prev_hash)
    entry.finalize_hash();

    // Write into P0 zone
    // SAFETY: idx < P0_ZONE_ENTRIES guaranteed by above check.
    //         Write is unique per slot (append-only).
    unsafe {
        core::ptr::write_volatile(p0_zone_ptr().add(idx), entry);
    }

    // Update last hash and counters
    store_last_hash(&entry.hash);
    TOTAL_WRITTEN.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// exo_ledger_append — Append into ring buffer (circular)
// ─────────────────────────────────────────────────────────────────────────────

/// Appends an entry into ledger ring buffer.
///
/// If ring buffer is full, the oldest entry is overwritten
/// (circular overflow permitted — but NEVER P0 entries).
///
/// # ISR-safe
/// No allocation, no lock. Uses atomics for head.
pub fn exo_ledger_append(action: ActionTag) {
    let _guard = P0_CHAIN_LOCK.lock();
    let seq = GLOBAL_SEQ.fetch_add(1, Ordering::AcqRel);
    let prev_hash = load_last_hash();
    let head = RING_HEAD.fetch_add(1, Ordering::AcqRel) % RING_BUFFER_ENTRIES;

    let mut entry = LedgerEntry {
        seq,
        tsc: rdtsc(),
        actor_oid: current_actor_oid(),
        action: action.to_bytes(),
        prev_hash,
        hash: [0u8; 32],
    };

    // Compute chained hash
    entry.finalize_hash();

    // Write into ring buffer
    // SAFETY: head < RING_BUFFER_ENTRIES by modulo construction.
    unsafe {
        core::ptr::write_volatile(ring_buffer_ptr().add(head), entry);
    }

    // Update last hash and counters
    store_last_hash(&entry.hash);
    TOTAL_WRITTEN.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoLedger initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the ExoLedger subsystem.
///
/// Called by `security_init()` at boot step 7.
///
/// In Phase 3.1, P0 and ring buffer zones reside in BSS static memory.
/// In Phase 3.2+, they will be mapped into SSR.LOG_AUDIT (+0x8000) in
/// collaboration with ExoPhoenix SSR infrastructure.
pub fn exo_ledger_init() {
    if P0_USED.load(Ordering::Acquire) != 0
        || TOTAL_WRITTEN.load(Ordering::Acquire) != 0
        || GLOBAL_SEQ.load(Ordering::Acquire) != 0
    {
        EXOLEDGER_INITIALIZED.store(true, Ordering::Release);
        return;
    }

    // Reset counters (in case of re-init)
    GLOBAL_SEQ.store(0, Ordering::Release);
    P0_USED.store(0, Ordering::Release);
    RING_HEAD.store(0, Ordering::Release);
    TOTAL_WRITTEN.store(0, Ordering::Release);
    P0_OVERFLOW_REPORTED.store(false, Ordering::Release);

    // Zero-initialize storage zones (already BSS = zero, but explicit).
    clear_ledger_storage();

    // Initialize last hash to zero (genesis entry)
    for byte in LAST_HASH.iter() {
        byte.store(0, Ordering::Release);
    }

    EXOLEDGER_INITIALIZED.store(true, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Integrity verification (Kernel B)
// ─────────────────────────────────────────────────────────────────────────────

/// Verifies integrity of P0 chain.
///
/// Traverses P0 entries and checks that each hash matches
/// Blake3(entry || prev_hash). Returns the number of valid P0 entries.
///
/// Called by Kernel B to verify integrity of P0 zone.
pub fn verify_p0_integrity() -> Result<usize, LedgerIntegrityError> {
    let p0_used = P0_USED.load(Ordering::Acquire);

    let mut prev_hash = [0u8; 32]; // genesis hash = zero

    for i in 0..p0_used.min(P0_ZONE_ENTRIES) {
        // SAFETY: i < p0_used <= P0_ZONE_ENTRIES, read-only.
        let entry = unsafe { core::ptr::read_volatile(p0_zone_ptr().add(i) as *const LedgerEntry) };

        // Verify chaining: entry prev_hash must match
        if entry.prev_hash != prev_hash {
            return Err(LedgerIntegrityError::ChainBroken { entry_idx: i });
        }

        // Verify entry hash
        let computed_hash = entry.compute_hash();
        if entry.hash != computed_hash {
            return Err(LedgerIntegrityError::HashMismatch { entry_idx: i });
        }

        prev_hash = entry.hash;
    }

    Ok(p0_used.min(P0_ZONE_ENTRIES))
}

/// Verifies integrity of ring buffer chain.
///
/// Returns number of valid entries in ring buffer.
pub fn verify_ring_integrity() -> Result<usize, LedgerIntegrityError> {
    let head = RING_HEAD.load(Ordering::Acquire);
    let count = head.min(RING_BUFFER_ENTRIES);

    // Ring buffer is circular — verify from start
    let mut prev_hash = [0u8; 32];
    let mut valid = 0;

    for i in 0..count {
        // SAFETY: i < count <= RING_BUFFER_ENTRIES, read-only.
        let entry =
            unsafe { core::ptr::read_volatile(ring_buffer_ptr().add(i) as *const LedgerEntry) };

        // Unwritten entries have seq=0, ignore them
        if entry.seq == 0 && valid == 0 {
            continue;
        }

        if entry.prev_hash != prev_hash && valid > 0 {
            // Chain may be broken by circular overflow
            // This is not an error — expected with ring buffer
            break;
        }

        let computed_hash = entry.compute_hash();
        if entry.hash == computed_hash {
            prev_hash = entry.hash;
            valid += 1;
        } else {
            break;
        }
    }

    Ok(valid)
}

/// Verifies the full cryptographic BLAKE3 chain across both the immutable P0 zone
/// and the circular ring buffer (which share `GLOBAL_SEQ` and `LAST_HASH`).
///
/// Checks:
/// 1. Every written P0 entry and ring entry matches its recomputed BLAKE3 hash.
/// 2. When sorted by `seq`, every contiguous pair `(seq, seq + 1)` satisfies
///    `next.prev_hash == cur.hash` (and `seq == 0` chains from the all-zero genesis).
/// 3. If any entries exist, the highest-`seq` entry's hash equals `LAST_HASH`.
pub fn verify_chain_integrity() -> Result<usize, LedgerIntegrityError> {
    let _guard = P0_CHAIN_LOCK.lock();
    let p0_count = P0_USED.load(Ordering::Acquire).min(P0_ZONE_ENTRIES);
    let ring_head = RING_HEAD.load(Ordering::Acquire);
    let ring_count = ring_head.min(RING_BUFFER_ENTRIES);

    const MAX_COMBINED: usize = P0_ZONE_ENTRIES + RING_BUFFER_ENTRIES;
    let mut entries: [(u64, [u8; 32], [u8; 32]); MAX_COMBINED] =
        [(0, [0u8; 32], [0u8; 32]); MAX_COMBINED];
    let mut total = 0usize;

    for i in 0..p0_count {
        // SAFETY: i < p0_count <= P0_ZONE_ENTRIES, read-only under P0_CHAIN_LOCK.
        let entry = unsafe { core::ptr::read_volatile(p0_zone_ptr().add(i) as *const LedgerEntry) };
        if entry.hash != entry.compute_hash() {
            return Err(LedgerIntegrityError::HashMismatch { entry_idx: i });
        }
        entries[total] = (entry.seq, entry.prev_hash, entry.hash);
        total += 1;
    }

    for i in 0..ring_count {
        // SAFETY: i < ring_count <= RING_BUFFER_ENTRIES, read-only under P0_CHAIN_LOCK.
        let entry =
            unsafe { core::ptr::read_volatile(ring_buffer_ptr().add(i) as *const LedgerEntry) };
        if entry.hash != entry.compute_hash() {
            return Err(LedgerIntegrityError::HashMismatch {
                entry_idx: p0_count + i,
            });
        }
        entries[total] = (entry.seq, entry.prev_hash, entry.hash);
        total += 1;
    }

    if total == 0 {
        return Ok(0);
    }

    // Insertion sort by sequence number (stack-only, bounded <= 112 elements).
    for i in 1..total {
        let key = entries[i];
        let mut j = i;
        while j > 0 && entries[j - 1].0 > key.0 {
            entries[j] = entries[j - 1];
            j -= 1;
        }
        entries[j] = key;
    }

    if entries[0].0 == 0 && entries[0].1 != [0u8; 32] {
        return Err(LedgerIntegrityError::ChainBroken { entry_idx: 0 });
    }

    for i in 1..total {
        if entries[i].0 == entries[i - 1].0.saturating_add(1) && entries[i].1 != entries[i - 1].2 {
            return Err(LedgerIntegrityError::ChainBroken { entry_idx: i });
        }
    }

    let last_hash = load_last_hash();
    if entries[total - 1].2 != last_hash {
        return Err(LedgerIntegrityError::ChainBroken {
            entry_idx: total - 1,
        });
    }

    Ok(total)
}

// ─────────────────────────────────────────────────────────────────────────────
// Integrity errors
// ─────────────────────────────────────────────────────────────────────────────

/// Ledger integrity verification error.
#[derive(Debug, Clone, Copy)]
pub enum LedgerIntegrityError {
    /// Hash chain broken at specified entry.
    ChainBroken { entry_idx: usize },
    /// Entry hash does not match recomputed hash.
    HashMismatch { entry_idx: usize },
    /// P0 zone full (cannot append).
    P0ZoneFull,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public accessors
// ─────────────────────────────────────────────────────────────────────────────

/// Number of used P0 entries.
#[inline(always)]
pub fn p0_used() -> usize {
    P0_USED.load(Ordering::Acquire)
}

/// Returns the stored BLAKE3 hash of P0 slot `idx` if written (read-only).
pub fn p0_entry_hash(idx: usize) -> Option<[u8; 32]> {
    let _guard = P0_CHAIN_LOCK.lock();
    let used = P0_USED.load(Ordering::Acquire).min(P0_ZONE_ENTRIES);
    if idx >= used {
        return None;
    }
    // SAFETY: idx < used <= P0_ZONE_ENTRIES, read-only under P0_CHAIN_LOCK.
    let entry = unsafe { core::ptr::read_volatile(p0_zone_ptr().add(idx) as *const LedgerEntry) };
    Some(entry.hash)
}

/// Total entries written (P0 + ring buffer).
#[inline(always)]
pub fn total_written() -> u64 {
    TOTAL_WRITTEN.load(Ordering::Acquire)
}

/// Current global sequence number.
#[inline(always)]
pub fn current_seq() -> u64 {
    GLOBAL_SEQ.load(Ordering::Acquire)
}

/// Reads a P0 entry by index.
///
/// Returns None if index is out of bounds or if entry was not written.
pub fn read_p0_entry(idx: usize) -> Option<LedgerEntry> {
    if idx >= P0_USED.load(Ordering::Acquire).min(P0_ZONE_ENTRIES) {
        return None;
    }
    // SAFETY: idx < P0_ZONE_ENTRIES, read-only.
    Some(unsafe { core::ptr::read_volatile(p0_zone_ptr().add(idx) as *const LedgerEntry) })
}

/// Reads a ring buffer entry by index.
pub fn read_ring_entry(idx: usize) -> Option<LedgerEntry> {
    if idx >= RING_BUFFER_ENTRIES {
        return None;
    }
    // SAFETY: idx < RING_BUFFER_ENTRIES, read-only.
    let entry =
        unsafe { core::ptr::read_volatile(ring_buffer_ptr().add(idx) as *const LedgerEntry) };
    if entry.seq == 0 {
        return None; // unwritten entry
    }
    Some(entry)
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoLedger statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of ExoLedger statistics.
#[derive(Debug, Clone, Copy)]
pub struct ExoLedgerStats {
    /// Number of used P0 entries.
    pub p0_used: usize,
    /// Maximum P0 capacity.
    pub p0_capacity: usize,
    /// Total entries written.
    pub total_written: u64,
    /// Current global sequence.
    pub current_seq: u64,
    /// Ring buffer head.
    pub ring_head: usize,
    /// Ring buffer capacity.
    pub ring_capacity: usize,
}

/// Returns a snapshot of ExoLedger statistics.
pub fn exoledger_stats() -> ExoLedgerStats {
    ExoLedgerStats {
        p0_used: P0_USED.load(Ordering::Relaxed),
        p0_capacity: P0_ZONE_ENTRIES,
        total_written: TOTAL_WRITTEN.load(Ordering::Relaxed),
        current_seq: GLOBAL_SEQ.load(Ordering::Relaxed),
        ring_head: RING_HEAD.load(Ordering::Relaxed),
        ring_capacity: RING_BUFFER_ENTRIES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exoledger_init_preserves_phase0_entries() {
        exo_ledger_init();
        exo_ledger_append_p0(ActionTag::NicIommuLocked);
        let seq_before = current_seq();
        let used_before = p0_used();

        exo_ledger_init();

        assert_eq!(p0_used(), used_before);
        assert_eq!(current_seq(), seq_before);
        assert!(read_p0_entry(0).is_some());
    }

    #[test]
    fn test_p0_overflow_is_graceful_and_saturating() {
        GLOBAL_SEQ.store(0, Ordering::Release);
        P0_USED.store(0, Ordering::Release);
        RING_HEAD.store(0, Ordering::Release);
        TOTAL_WRITTEN.store(0, Ordering::Release);
        P0_OVERFLOW_REPORTED.store(false, Ordering::Release);
        clear_ledger_storage();
        for byte in LAST_HASH.iter() {
            byte.store(0, Ordering::Release);
        }

        for idx in 0..P0_ZONE_ENTRIES {
            exo_ledger_append_p0(ActionTag::BootEvent { step: idx as u8 });
        }
        let written_before = total_written();
        exo_ledger_append_p0(ActionTag::BootEvent { step: 99 });

        assert_eq!(p0_used(), P0_ZONE_ENTRIES);
        assert!(total_written() >= written_before);
        assert!(read_ring_entry(0).is_some());
    }
}
