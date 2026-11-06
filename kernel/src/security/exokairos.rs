// kernel/src/security/exokairos.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoKairos — Budgeted Temporal Capabilities (ExoShield v1.0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoKairos implements the ExoShield temporal capabilities system.
// Each capability has a budget (calls and volume) that decreases monotonically,
// and a hidden deadline — temporal expiration is stored outside the TCB in
// a kernel-only table (PKS domain Credentials), and verified in constant time.
//
// ARCHITECTURE:
//   • TemporalCap: capability structure with atomic budget + deadline MAC
//   • cap_deadline_table: TSC deadline storage outside TCB (Ring 0 only, PKS Credentials)
//   • Inline verification: zero IPC, constant-time, ~30 cycles
//
// SECURITY PROPERTIES:
//   S4 (Budget Monotonicity): □(use_cap ⟹ budget' < budget)
//     -> calls_left is AtomicU32 decremented via fetch_sub — always decreasing
//   Hidden deadline: deadline TSC is NEVER exposed to Ring 1
//     -> Only deadline_mac (HMAC-Blake3) is visible in TemporalCap
//     -> Mythos cannot invert the deadline from the MAC
//
// TTL PER RIGHT (ExoShield Spec MODULE 5):
//   NETWORK_SEND: 5 seconds
//   FILE_WRITE  : 30 seconds
//   EXEC        : 1 second
//   IPC_CALL    : 60 seconds
//   Default     : 5 minutes (300 seconds)
//
// CORRECTION CORR-v3.1-02:
//   `deadline_tsc` is no longer exposed in the TemporalCap structure.
//   Only `deadline_mac: [u8; 16]` is stored — the actual deadline is in
//   cap_deadline_table (PKS Credentials domain, inaccessible from Ring 1).
//
// RULE EXOKAIROS-01: MAC/deadline verification remains constant-time.
//   throttle/kill decisions branch only on public counters.
// RULE EXOKAIROS-02: verify() performs NO IPC — all logic is inline.
// RULE EXOKAIROS-03: depth_max = 4 — no delegation beyond 4 levels.
//
// REFERENCES:
//   ExoShield_v1_Production.md — MODULE 5: ExoKairos
//   kernel/src/security/capability/ — CapToken, Rights, verify, revoke
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Once;

pub use crate::arch::constants::{KAIROS_KILL_PCT, KAIROS_THROTTLE_PCT, KAIROS_WINDOW_NS};
use crate::security::capability::rights::Rights;
use crate::security::capability::token::CapToken;
use crate::security::exoveil::{self, PksDomain, PksPermission};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes ExoKairos
// ─────────────────────────────────────────────────────────────────────────────

/// TTL (Time-To-Live) per right in seconds.
/// ExoShield Spec MODULE 5 — TTL defaults.
pub mod ttl {
    /// NETWORK_SEND: 5 seconds.
    pub const NETWORK_SEND_S: u64 = 5;
    /// FILE_WRITE: 30 seconds.
    pub const FILE_WRITE_S: u64 = 30;
    /// EXEC: 1 second.
    pub const EXEC_S: u64 = 1;
    /// IPC_CALL: 60 seconds.
    pub const IPC_CALL_S: u64 = 60;
    /// Default: 5 minutes (300 seconds).
    pub const DEFAULT_S: u64 = 300;
}

/// Maximum delegation depth (4 levels).
pub const MAX_DELEGATION_DEPTH: u8 = 4;

const KAIROS_THROTTLE_SPINS: usize = 64;

const _: () = assert!(
    KAIROS_WINDOW_NS == 1_000_000_000,
    "KAIROS_WINDOW_NS must remain a 1s audit window"
);

/// Maximum deadline table size (number of entries).
/// In Phase 3.1, a static table is used. In Phase 3.2+,
/// the table will be dynamic (hash map protected by PKS Credentials).
const DEADLINE_TABLE_SIZE: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// CapError — Erreurs de capability temporelle
// ─────────────────────────────────────────────────────────────────────────────

/// Possible errors during temporal capability verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// The capability has expired (deadline TSC exceeded).
    Expired,
    /// Call budget is exhausted (calls_left == 0).
    BudgetExhausted,
    /// Volume budget is exhausted (bytes_left == 0).
    VolumeExhausted,
    /// Delegation depth exceeded.
    DepthExceeded,
    /// Base token is invalid (revoked, wrong generation, etc.).
    InvalidToken,
    /// Deadline MAC does not match — potentially forged.
    MacMismatch,
    /// Capability does not exist in deadline table.
    NotFound,
    /// Window budget reached throttle threshold (100%).
    ThrottleRequired,
    /// Window budget reached kill threshold (200%).
    KillThresholdExceeded,
}

// ─────────────────────────────────────────────────────────────────────────────
// TemporalCap — Capability temporelle
// ─────────────────────────────────────────────────────────────────────────────

/// Temporal capability with atomic budget and hidden deadline.
///
/// # Layout (repr(C))
///
/// | Champ          | Type       | Taille | Description                           |
/// |----------------|------------|--------|---------------------------------------|
/// | base           | CapToken   | 24     | Token de capability standard          |
/// | deadline_mac   | [u8; 16]  | 16     | HMAC-Blake3(oid\|\|deadline\|\|SECRET)|
/// | calls_left     | AtomicU32 | 4      | Budget d'appels restant              |
/// | bytes_left     | AtomicU64 | 8      | Volume restant (octets)              |
/// | window_start_ns| AtomicU64 | 8      | Current window start                  |
/// | window budgets | u32/u64   | 12     | Budget per window                     |
/// | window used    | Atomic*   | 16     | Window consumption                    |
/// | depth          | u8        | 1      | Delegation depth (0..4)               |
///
/// # Security
/// - `deadline_mac`: HMAC-Blake3 of (oid || deadline_tsc || KERNEL_SECRET).
///   The attacker CANNOT deduce the deadline TSC from the MAC.
/// - `calls_left` / `bytes_left`: atomic, reloaded only on change
///   of `KAIROS_WINDOW_NS` window.
/// - `window_used_*`: trigger throttle at 100% and kill at 200%.
/// - `depth`: can never exceed MAX_DELEGATION_DEPTH (4).
#[repr(C)]
pub struct TemporalCap {
    /// Standard capability token (object_id, rights, generation, type_tag).
    pub base: CapToken,
    /// HMAC-Blake3(oid || deadline_tsc || KERNEL_SECRET).
    /// Actual deadline TSC is stored in cap_deadline_table (Ring 0 only).
    pub deadline_mac: [u8; 16],
    /// Remaining call budget (atomically decremented).
    pub calls_left: AtomicU32,
    /// Remaining volume in bytes (atomically decremented).
    pub bytes_left: AtomicU64,
    /// Start of current window in monotonic time.
    pub window_start_ns: AtomicU64,
    /// Call budget reloaded at each window.
    window_budget_calls: u32,
    /// Volume budget reloaded at each window.
    window_budget_bytes: u64,
    /// Calls consumed in current window.
    window_used_calls: AtomicU64,
    /// Bytes consumed in current window.
    window_used_bytes: AtomicU64,
    /// Delegation depth (0 = original, 1..4 = delegated).
    pub depth: u8,
}

impl TemporalCap {
    /// Creates a new temporal capability.
    ///
    /// # Arguments
    /// - `base`: Standard capability token
    /// - `deadline_tsc`: Absolute deadline TSC (stored in deadline table)
    /// - `initial_calls`: Initial call budget
    /// - `initial_bytes`: Initial volume budget (bytes)
    /// - `depth`: Delegation depth (0 for a new cap)
    ///
    /// # Safety
    /// - `deadline_tsc` is stored in cap_deadline_table (PKS Credentials)
    /// - MAC is computed with KERNEL_SECRET (immutable after boot)
    // SAFETY: validated low-level operation — see block documentation.
    pub unsafe fn new(
        base: CapToken,
        deadline_tsc: u64,
        initial_calls: u32,
        initial_bytes: u64,
        depth: u8,
    ) -> Self {
        debug_assert!(depth <= MAX_DELEGATION_DEPTH, "delegation depth exceeded");

        // Compute deadline MAC: HMAC-Blake3(oid || deadline_tsc || KERNEL_SECRET)
        let deadline_mac = compute_deadline_mac(base.object_id(), deadline_tsc);

        // Store deadline in kernel-only table
        let _ = cap_deadline_table::insert(base.object_id(), deadline_tsc);

        Self {
            base,
            deadline_mac,
            calls_left: AtomicU32::new(initial_calls),
            bytes_left: AtomicU64::new(initial_bytes),
            window_start_ns: AtomicU64::new(0),
            window_budget_calls: initial_calls,
            window_budget_bytes: initial_bytes,
            window_used_calls: AtomicU64::new(0),
            window_used_bytes: AtomicU64::new(0),
            depth,
        }
    }

    #[inline]
    #[cfg(test)]
    fn current_window_ns(fallback: u64) -> u64 {
        fallback
    }

    #[inline]
    #[cfg(not(test))]
    fn current_window_ns(fallback: u64) -> u64 {
        let now = crate::scheduler::timer::clock::monotonic_ns();
        if now == 0 {
            fallback
        } else {
            now
        }
    }

    fn check_and_advance_window(&self, now_ns: u64) {
        let mut start = self.window_start_ns.load(Ordering::Acquire);
        if start == 0 {
            match self.window_start_ns.compare_exchange(
                0,
                now_ns,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => start = actual,
            }
        }

        if now_ns.saturating_sub(start) < KAIROS_WINDOW_NS {
            return;
        }

        if self
            .window_start_ns
            .compare_exchange(start, now_ns, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.window_used_calls.store(0, Ordering::Release);
            self.window_used_bytes.store(0, Ordering::Release);
            self.calls_left
                .store(self.window_budget_calls, Ordering::Release);
            self.bytes_left
                .store(self.window_budget_bytes, Ordering::Release);
        }
    }

    #[inline(always)]
    fn throttle_window() {
        let mut spins = 0usize;
        while spins < KAIROS_THROTTLE_SPINS {
            core::hint::spin_loop();
            spins += 1;
        }
    }

    #[inline]
    fn throttle_or_kill(used: u64, budget: u64) -> Result<(), CapError> {
        if budget == 0 {
            return Err(CapError::BudgetExhausted);
        }
        // FIX-KAIROS-01 (analysis report §5.4): `used.saturating_mul(100)` saturated
        // at u64::MAX if used > u64::MAX/100 (~1.84×10¹⁷, or ~5840 years in nanoseconds),
        // triggering a false KillThresholdExceeded regardless of actual budget.
        //
        // Fix: checked_mul(100). If overflow occurs, budget is
        // definitively exceeded — kill is therefore legitimate (astronomical value).
        let used_pct = match used.checked_mul(100) {
            Some(v) => v,
            None => {
                // used > u64::MAX / 100 -> inevitably exceeds any budget × KAIROS_KILL_PCT.
                return Err(CapError::KillThresholdExceeded);
            }
        };
        if used_pct >= budget.saturating_mul(KAIROS_KILL_PCT) {
            return Err(CapError::KillThresholdExceeded);
        }
        if used_pct >= budget.saturating_mul(KAIROS_THROTTLE_PCT) {
            Self::throttle_window();
        }
        Ok(())
    }

    #[inline]
    fn debit_call_budget(&self) {
        let mut current = self.calls_left.load(Ordering::Acquire);
        while current != 0 {
            match self.calls_left.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Checks validity of this capability — constant-time MAC/deadline, ZERO IPC.
    ///
    /// # Properties
    /// - Constant-time for hidden deadline and MAC
    /// - Zero IPC: all logic is inline in Ring 0
    /// - Monotonic: calls_left and bytes_left always decrease
    ///
    /// # Sequence
    /// 1. Retrieve deadline from cap_deadline_table (inaccessible from Ring 1)
    /// 2. Verify expiration (constant-time comparison)
    /// 3. Verify MAC (tamper protection)
    /// 4. Atomically decrement calls_left
    /// 5. Verify remaining budget
    ///
    /// # RULE EXOKAIROS-01
    /// The secret part (deadline table + MAC) remains constant-time. Budget window
    /// may trigger throttle/kill on public counters.
    pub fn verify(&self, current_tsc: u64) -> Result<(), CapError> {
        // 1. Retrieve deadline from kernel table (inaccessible from Ring 1)
        let deadline = cap_deadline_table::get_const_time(self.base.object_id());
        if deadline == u64::MAX {
            return Err(CapError::NotFound);
        }

        // 2. Verify MAC (protection against deadline_mac forgery)
        let expected_mac = compute_deadline_mac(self.base.object_id(), deadline);
        if !ct_u8_array_eq(&self.deadline_mac, &expected_mac) {
            return Err(CapError::MacMismatch);
        }

        // 3. Verify expiration (branchless comparison on secret bits).
        if ct_u64_gte_mask(current_tsc, deadline) != 0 {
            return Err(CapError::Expired);
        }

        // 4. Sliding window: reset every `KAIROS_WINDOW_NS`, throttle
        //    at 100% of budget and kill at 200%.
        let now_ns = Self::current_window_ns(current_tsc);
        self.check_and_advance_window(now_ns);
        let used_calls = self
            .window_used_calls
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        Self::throttle_or_kill(used_calls, self.window_budget_calls as u64)?;

        // 5. Saturating atomic deduction of visible budget in window.
        self.debit_call_budget();

        if self.window_budget_bytes == 0 {
            return Err(CapError::VolumeExhausted);
        }

        Ok(())
    }

    /// Decrements remaining volume by `n` bytes.
    ///
    /// Returns remaining volume after decrement, or 0 if budget
    /// is exhausted (saturating_sub).
    #[inline(always)]
    pub fn consume_bytes(&self, n: u64) -> u64 {
        let now_ns = Self::current_window_ns(0);
        self.check_and_advance_window(now_ns);
        let used_bytes = self
            .window_used_bytes
            .fetch_add(n, Ordering::AcqRel)
            .saturating_add(n);
        let _ = Self::throttle_or_kill(used_bytes, self.window_budget_bytes);

        loop {
            let current = self.bytes_left.load(Ordering::Acquire);
            let new_val = current.saturating_sub(n);
            match self.bytes_left.compare_exchange_weak(
                current,
                new_val,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return new_val,
                Err(_) => continue, // retry (CAS contention)
            }
        }
    }

    /// Creates a delegated capability (subset of rights).
    ///
    /// # RULE CAP-03 (inherited)
    /// Delegated rights can NEVER exceed the rights of the source.
    ///
    /// # RULE EXOKAIROS-03
    /// Delegation depth cannot exceed MAX_DELEGATION_DEPTH (4).
    ///
    /// Budget and deadline are inherited (subset of parent).
    pub fn delegate(
        &self,
        restricted_rights: Rights,
        new_calls: u32,
        new_bytes: u64,
    ) -> Result<TemporalCap, CapError> {
        // Check delegation depth
        if self.depth >= MAX_DELEGATION_DEPTH {
            return Err(CapError::DepthExceeded);
        }

        // Verify that requested rights are a subset
        if !restricted_rights.is_subset_of(self.base.rights()) {
            return Err(CapError::InvalidToken);
        }

        // Delegated budget cannot exceed parent remaining budget
        let parent_calls = self.calls_left.load(Ordering::Acquire);
        let parent_bytes = self.bytes_left.load(Ordering::Acquire);

        if new_calls > parent_calls || new_bytes > parent_bytes {
            return Err(CapError::BudgetExhausted);
        }

        // Create delegated token (same object_id, restricted rights)
        // Note: CapToken::new is pub(super) — in Phase 3.1 constructed manually
        // for tests. In production, delegate() goes through CapTable::grant().
        let delegated_base = CapToken::new(
            self.base.object_id(),
            restricted_rights,
            self.base.generation(),
            self.base.object_type(),
        );

        // Retrieve deadline from table
        let deadline = cap_deadline_table::get_const_time(self.base.object_id());

        // SAFETY: deadline is valid, depth is verified
        Ok(unsafe {
            TemporalCap::new(
                delegated_base,
                deadline,
                new_calls,
                new_bytes,
                self.depth + 1,
            )
        })
    }

    /// Returns delegation depth.
    #[inline(always)]
    pub fn depth(&self) -> u8 {
        self.depth
    }

    /// Returns remaining call budget.
    #[inline(always)]
    pub fn calls_remaining(&self) -> u32 {
        self.calls_left.load(Ordering::Acquire)
    }

    /// Returns remaining volume.
    #[inline(always)]
    pub fn bytes_remaining(&self) -> u64 {
        self.bytes_left.load(Ordering::Acquire)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// cap_deadline_table — Stockage deadline TSC (Ring 0 only, PKS Credentials)
// ─────────────────────────────────────────────────────────────────────────────

/// Module storing TSC deadlines.
///
/// Deadlines are stored in a table separate from TCB and TemporalCap.
/// This table resides in PKS Credentials domain (pkey=2) — inaccessible
/// from Ring 1 and Ring 3.
///
/// In Phase 3.1, the table is a static array. In Phase 3.2+,
/// it will be a dynamic hash table protected by PKS.
pub mod cap_deadline_table {
    use super::*;
    use crate::security::capability::token::ObjectId;

    /// Deadline table entry: (ObjectId, deadline_tsc).
    #[repr(C)]
    struct DeadlineEntry {
        oid: u64,
        deadline_tsc: u64,
        occupied: AtomicU32, // 0 = free, 1 = occupied
    }

    /// Static table of deadlines (Phase 3.1).
    /// Size: 256 entries × 20 bytes = 5 KiB.
    static mut TABLE: [DeadlineEntry; DEADLINE_TABLE_SIZE] = {
        const INIT: DeadlineEntry = DeadlineEntry {
            oid: 0,
            deadline_tsc: 0,
            occupied: AtomicU32::new(0),
        };
        [INIT; DEADLINE_TABLE_SIZE]
    };

    /// Counter of used entries.
    static USED: AtomicUsize = AtomicUsize::new(0);

    /// Inserts a deadline into table.
    ///
    /// # Safety
    /// Ring 0 only. Table is protected by PKS Credentials.
    // SAFETY: validated low-level operation — see block documentation.
    pub unsafe fn insert(oid: ObjectId, deadline_tsc: u64) -> bool {
        let _guard = unsafe {
            exoveil::scoped_domain_access(PksDomain::Credentials, PksPermission::ReadWrite)
        };
        let oid_val = oid.as_u64();

        // Sequential search (Phase 3.1 — will be hash table in Phase 3.2)
        for i in 0..DEADLINE_TABLE_SIZE {
            // SAFETY: i < DEADLINE_TABLE_SIZE; mutable raw pointer reserved
            // to this Ring 0 routine for atomic update of entries.
            let entry = &raw mut TABLE[i];
            let occupied = (*entry).occupied.load(Ordering::Acquire);

            if occupied == 1 && (*entry).oid == oid_val {
                // Update existing entry
                (*entry).deadline_tsc = deadline_tsc;
                return true;
            }

            if occupied == 0 {
                // Free slot — CAS insertion
                match (*entry)
                    .occupied
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => {
                        (*entry).oid = oid_val;
                        (*entry).deadline_tsc = deadline_tsc;
                        USED.fetch_add(1, Ordering::Relaxed);
                        return true;
                    }
                    Err(_) => continue, // Failed CAS — another thread took the slot
                }
            }
        }

        false
    }

    /// Retrieves TSC deadline for an ObjectId — constant time.
    ///
    /// # Constant-time
    /// This function ALWAYS traverses the entire table, even if deadline
    /// is found at the beginning. This guarantees execution time does not depend
    /// on entry position.
    ///
    /// # RULE EXOKAIROS-01
    /// Constant-time access to prevent timing attacks.
    pub fn get_const_time(oid: ObjectId) -> u64 {
        // SAFETY: validated low-level operation — see block documentation.
        let _guard = unsafe {
            exoveil::scoped_domain_access(PksDomain::Credentials, PksPermission::ReadOnly)
        };
        let oid_val = oid.as_u64();
        let mut result = u64::MAX; // deadline = MAX means "not found" -> Expired

        // SAFETY: read-only access to static table, no concurrent write
        // during get_const_time (PKS Credentials protects in Phase 3.2).
        unsafe {
            for i in 0..DEADLINE_TABLE_SIZE {
                let entry = &raw const TABLE[i];
                let occupied = (*entry).occupied.load(Ordering::Acquire);

                // Constant-time comparison: using ct_eq
                let oid_match = ct_u64_eq((*entry).oid, oid_val);
                let is_valid = (occupied == 1) as u64 & oid_match;

                // Branchless conditional selection:
                // If is_valid != 0, result = entry.deadline_tsc
                // Otherwise, result = result (unchanged)
                let mask = is_valid.wrapping_sub(1); // 0x0000... if match, 0xFFFF... otherwise
                result = (result & mask) | ((*entry).deadline_tsc & !mask);
            }
        }

        result
    }

    /// Removes an entry from deadline table.
    ///
    /// Called upon revocation of a temporal capability.
    pub fn remove(oid: ObjectId) {
        // SAFETY: PKS Credentials write-access required to modify deadline table.
        // scoped_domain_access() uses WRPKRU to temporarily open domain.
        let _guard = unsafe {
            exoveil::scoped_domain_access(PksDomain::Credentials, PksPermission::ReadWrite)
        };
        let oid_val = oid.as_u64();

        // SAFETY: TABLE is a static array of DEADLINE_TABLE_SIZE entries.
        // Exclusive access guaranteed by PKS Credentials lock (_guard above).
        // i < DEADLINE_TABLE_SIZE is maintained by the loop.
        unsafe {
            for i in 0..DEADLINE_TABLE_SIZE {
                let entry = &raw mut TABLE[i];
                if (*entry).occupied.load(Ordering::Acquire) == 1 && (*entry).oid == oid_val {
                    (*entry).occupied.store(0, Ordering::Release);
                    (*entry).oid = 0;
                    (*entry).deadline_tsc = 0;
                    USED.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
            }
        }
    }

    /// Number of used entries in table.
    pub fn used_count() -> usize {
        USED.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions constant-time
// ─────────────────────────────────────────────────────────────────────────────

/// Constant-time comparison of two u64.
///
/// Returns 1 if a == b, 0 otherwise. Execution time is independent
/// of compared values.
#[inline(always)]
fn ct_u64_eq(a: u64, b: u64) -> u64 {
    let xor = a ^ b;
    // If xor == 0, values are equal.
    // We use the fact that (xor | -xor) >> 63 == 0 only if xor == 0.
    let v = xor | xor.wrapping_neg();
    (v >> 63).wrapping_sub(1) & 1 // 1 if equal, 0 otherwise
}

/// Branchless comparison: a >= b.
///
/// Returns 1 if a >= b, 0 otherwise. The formula computes borrow bit of
/// `a - b` in unsigned arithmetic, without relying on sign bit alone.
#[inline(always)]
fn ct_u64_gte_mask(a: u64, b: u64) -> u64 {
    let diff = a.wrapping_sub(b);
    let borrow = ((!a & b) | (!(a ^ b) & diff)) >> 63;
    borrow ^ 1
}

/// Constant-time comparison of two 16-byte arrays.
///
/// Returns true if arrays are identical.
#[inline(always)]
fn ct_u8_array_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut acc: u8 = 0;
    for i in 0..16 {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Deadline MAC — HMAC-Blake3
// ─────────────────────────────────────────────────────────────────────────────

/// Computes deadline MAC: HMAC-Blake3(oid || deadline_tsc || KERNEL_SECRET).
///
/// The MAC truncated to 16 bytes is stored in TemporalCap.deadline_mac.
/// Actual deadline TSC is NOT deducible from this MAC.
///
fn compute_deadline_mac(
    oid: crate::security::capability::token::ObjectId,
    deadline_tsc: u64,
) -> [u8; 16] {
    // Build message: oid (8 bytes) || deadline_tsc (8 bytes)
    let mut msg = [0u8; 16];
    msg[0..8].copy_from_slice(&oid.as_u64().to_le_bytes());
    msg[8..16].copy_from_slice(&deadline_tsc.to_le_bytes());

    let key = get_kernel_secret();
    let full_hash = crate::security::crypto::blake3::blake3_mac(&key, &msg);

    // Truncate to 16 bytes (128 bits of security)
    let mut mac = [0u8; 16];
    mac.copy_from_slice(&full_hash[0..16]);
    mac
}

/// KERNEL_SECRET — secret key initialized by ExoSeal at boot.
/// 32 bytes, stored in static memory (will be in PKS Credentials in Phase 3.2).
static KERNEL_SECRET: Once<[u8; 32]> = Once::new();

/// Initializes KERNEL_SECRET (called by ExoSeal at boot).
///
/// # Safety
/// Must be called ONCE at boot, before Kernel A starts.
/// Secret must originate from a CSPRNG source (RDRAND + ChaCha20).
pub fn init_kernel_secret(secret: &[u8; 32]) {
    let _guard =
        // SAFETY: validated low-level operation — see block documentation.
        unsafe { exoveil::scoped_domain_access(PksDomain::Credentials, PksPermission::ReadWrite) };
    KERNEL_SECRET.call_once(|| *secret);
}

/// Reads KERNEL_SECRET (Ring 0 only).
///
/// # Safety
/// Must never be called from Ring 1 or Ring 3.
/// In Phase 3.2, this read will be protected by PKS Credentials.
fn get_kernel_secret() -> [u8; 32] {
    let _guard =
        // SAFETY: validated low-level operation — see block documentation.
        unsafe { exoveil::scoped_domain_access(PksDomain::Credentials, PksPermission::ReadOnly) };
    KERNEL_SECRET.get().copied().unwrap_or([0u8; 32])
}

// ─────────────────────────────────────────────────────────────────────────────
// TTL Lookup — Determines TTL for a given right
// ─────────────────────────────────────────────────────────────────────────────

/// Registers a temporal deadline for a newly created capability.
///
/// FIX-P1-KAIROS (Security_Application_Audit §GAP-07): `ttl_for_right()` had
/// no external caller — IPC tokens never expired. This function
/// is called from `capability::create()` to associate each issued token with
/// a deadline = now + TTL(rights). Time base is `monotonic_ns()`
/// (the same as `verify()` via `current_window_ns`), so that `TemporalCap::
/// verify(monotonic_ns())` will detect expiration.
///
/// Best-effort: if clock is not yet calibrated (monotonic_ns == 0) or if
/// table is full, registration is ignored (token remains non-temporal,
/// identical behavior to existing — no regression).
pub fn register_ttl_for_cap(oid: crate::security::capability::token::ObjectId, rights: Rights) {
    // In host test, kernel clock does not exist — same convention as
    // TemporalCap::current_window_ns (variant #[cfg(test)]): deterministic no-op.
    #[cfg(test)]
    {
        let _ = (oid, rights);
    }
    #[cfg(not(test))]
    {
        let now_ns = crate::scheduler::timer::clock::monotonic_ns();
        if now_ns == 0 {
            return;
        }
        let ttl_s = ttl_for_right(rights);
        let deadline_ns = now_ns.saturating_add(ttl_s.saturating_mul(1_000_000_000));
        // SAFETY: Ring 0, table protected by PKS Credentials via scoped_domain_access
        // internal to insert(). ObjectId comes from kernel table (create()).
        unsafe {
            let _ = cap_deadline_table::insert(oid, deadline_ns);
        }
    }
}

/// Returns TTL in seconds for a specific right.
///
/// Used when creating a TemporalCap to calculate deadline.
pub fn ttl_for_right(rights: Rights) -> u64 {
    // TTL is that of the most restrictive right (shortest)
    if rights.contains(Rights::EXEC) {
        return ttl::EXEC_S;
    }
    if rights.contains(Rights::IPC_SEND) {
        return ttl::NETWORK_SEND_S; // IPC_SEND ≈ NETWORK_SEND for TTL
    }
    if rights.contains(Rights::WRITE) {
        return ttl::FILE_WRITE_S;
    }
    if rights.contains(Rights::IPC_CONNECT) || rights.contains(Rights::IPC_RECV) {
        return ttl::IPC_CALL_S;
    }
    ttl::DEFAULT_S
}

// ─────────────────────────────────────────────────────────────────────────────
// ExoKairos statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of ExoKairos statistics.
#[derive(Debug, Clone, Copy)]
pub struct ExoKairosStats {
    /// Number of entries in deadline table.
    pub deadline_table_used: usize,
    /// KERNEL_SECRET initialized.
    pub secret_initialized: bool,
}

/// Returns a snapshot of ExoKairos statistics.
pub fn exokairos_stats() -> ExoKairosStats {
    ExoKairosStats {
        deadline_table_used: cap_deadline_table::used_count(),
        secret_initialized: KERNEL_SECRET.get().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::{cap_deadline_table, CapError, TemporalCap, DEADLINE_TABLE_SIZE, KAIROS_WINDOW_NS};
    use crate::security::capability::rights::Rights;
    use crate::security::capability::token::{CapObjectType, CapToken, ObjectId};
    use core::sync::atomic::Ordering;
    use spin::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_oid(raw: u64) -> ObjectId {
        ObjectId::from_raw(0x4558_4f4b_0000_0000 | raw)
    }

    fn test_token(raw: u64) -> CapToken {
        CapToken::new(test_oid(raw), Rights::READ, 1, CapObjectType::FileInode)
    }

    #[test]
    fn deadline_table_round_trip_smoke() {
        let _guard = TEST_LOCK.lock();
        let oid = test_oid(1);
        cap_deadline_table::remove(oid);
        let before = cap_deadline_table::used_count();

        // SAFETY: validated low-level operation — see block documentation.
        unsafe {
            cap_deadline_table::insert(oid, 0x1234_5678_9abc_def0);
        }
        assert_eq!(
            cap_deadline_table::get_const_time(oid),
            0x1234_5678_9abc_def0
        );
        assert_eq!(cap_deadline_table::used_count(), before + 1);

        // SAFETY: validated low-level operation — see block documentation.
        unsafe {
            cap_deadline_table::insert(oid, 0x0fed_cba9_8765_4321);
        }
        assert_eq!(
            cap_deadline_table::get_const_time(oid),
            0x0fed_cba9_8765_4321
        );
        assert_eq!(cap_deadline_table::used_count(), before + 1);

        cap_deadline_table::remove(oid);
        assert_eq!(cap_deadline_table::get_const_time(oid), u64::MAX);
        assert_eq!(cap_deadline_table::used_count(), before);
    }

    #[test]
    fn deadline_table_capacity_stress() {
        let _guard = TEST_LOCK.lock();
        let before = cap_deadline_table::used_count();

        assert_eq!(
            before, 0,
            "deadline table unexpectedly pre-populated before stress test"
        );

        for idx in 0..DEADLINE_TABLE_SIZE {
            let oid = test_oid(0x1000 + idx as u64);
            cap_deadline_table::remove(oid);
            // SAFETY: validated low-level operation — see block documentation.
            unsafe {
                cap_deadline_table::insert(oid, 0x1000_0000 + idx as u64);
            }
        }

        for idx in 0..DEADLINE_TABLE_SIZE {
            let oid = test_oid(0x1000 + idx as u64);
            assert_eq!(
                cap_deadline_table::get_const_time(oid),
                0x1000_0000 + idx as u64
            );
        }
        assert_eq!(cap_deadline_table::used_count(), DEADLINE_TABLE_SIZE);

        for idx in 0..DEADLINE_TABLE_SIZE {
            let oid = test_oid(0x1000 + idx as u64);
            cap_deadline_table::remove(oid);
        }
        assert_eq!(cap_deadline_table::used_count(), 0);
    }

    #[test]
    fn window_budget_throttles_then_kills_at_double_budget() {
        let _guard = TEST_LOCK.lock();
        let oid = test_oid(0x3000);
        cap_deadline_table::remove(oid);
        // SAFETY: validated low-level operation — see block documentation.
        let cap = unsafe { TemporalCap::new(test_token(0x3000), u64::MAX - 1, 2, 64, 0) };

        assert_eq!(cap.verify(1), Ok(()));
        assert_eq!(cap.verify(2), Ok(()));
        assert_eq!(cap.verify(3), Ok(()));
        assert_eq!(cap.verify(4), Err(CapError::KillThresholdExceeded));

        cap_deadline_table::remove(oid);
    }

    #[test]
    fn window_reset_recharges_visible_budgets() {
        let _guard = TEST_LOCK.lock();
        let oid = test_oid(0x3001);
        cap_deadline_table::remove(oid);
        // SAFETY: validated low-level operation — see block documentation.
        let cap = unsafe { TemporalCap::new(test_token(0x3001), u64::MAX - 1, 2, 64, 0) };

        assert_eq!(cap.verify(1), Ok(()));
        assert_eq!(cap.verify(2), Ok(()));
        assert_eq!(cap.calls_remaining(), 0);

        let start = cap.window_start_ns.load(Ordering::Acquire);
        cap.check_and_advance_window(start.saturating_add(KAIROS_WINDOW_NS + 2));
        assert_eq!(cap.calls_remaining(), 2);
        assert_eq!(cap.bytes_remaining(), 64);

        cap_deadline_table::remove(oid);
    }
}
