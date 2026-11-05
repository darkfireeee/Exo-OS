// kernel/src/memory/integrity/canary.rs
//
// Stack canary — generation, per-CPU storage and verification.
//
// Principle:
//   - At init, a pseudo-random value is generated via RDTSC + XOR.
//   - This value is placed into the table CANARY_TABLE[cpu_id] (read-only
//     after init).
//   - The prologue of each instrumented function reads the current value and
//     pushes it onto its stack frame. The epilogue re-checks.
//   - In case of divergence -> `canary_violation_handler`.
//
// Exo-OS adds a second level: "thread canary" stored in the TCB,
// initialized with the value cpu_canary XOR tid.
//
// LAYER 0 — no dependency on scheduler/process/ipc/fs.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
// FIX-CANARY-01: import MAX_CPUS from canonical source.
// Prevents silent divergence if MAX_CPUS changes in constants.rs.
use crate::memory::core::constants::MAX_CPUS;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinel value indicating table is not yet initialized.
const CANARY_UNINIT: u64 = 0xDEAD_BEEF_CAFE_BABE;
/// Poison to detect stack reuse after violation.
#[allow(dead_code)]
const CANARY_POISON: u64 = 0x0000_0000_0000_0000;

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct CanaryStats {
    /// Number of initialized canaries (BSP + APs).
    pub init_count: AtomicU64,
    /// Detected violations.
    pub violation_count: AtomicU64,
    /// Forced regenerations (key rotation).
    pub rotate_count: AtomicU64,
    /// Successful checks (debug metrics, expensive in prod).
    pub check_ok: AtomicU64,
}

impl Default for CanaryStats {
    fn default() -> Self {
        Self::new()
    }
}

impl CanaryStats {
    const fn new() -> Self {
        Self {
            init_count: AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            rotate_count: AtomicU64::new(0),
            check_ok: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for CanaryStats {}
pub static CANARY_STATS: CanaryStats = CanaryStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU Canary Table
// ─────────────────────────────────────────────────────────────────────────────

/// A slot in the canary table.
/// `align(64)` avoids false sharing between adjacent CPUs.
#[repr(C, align(64))]
struct CanarySlot {
    value: AtomicU64,
    generation: AtomicU64,
    initialized: AtomicBool,
    _pad: [u8; 39],
}

impl CanarySlot {
    #[allow(dead_code)]
    const fn uninit() -> Self {
        Self {
            value: AtomicU64::new(CANARY_UNINIT),
            generation: AtomicU64::new(0),
            initialized: AtomicBool::new(false),
            _pad: [0u8; 39],
        }
    }
}

/// CPU canary table — MAX_CPUS * 64 bytes = 16 KiB.
struct CanaryTable {
    slots: [CanarySlot; MAX_CPUS],
}

/// # SAFETY: access by exclusive cpu_id -> Sync guaranteed by usage protocol.
unsafe impl Sync for CanaryTable {}

/// Const construction of the table.
/// Stable Rust does not yet support `[CanarySlot::uninit(); N]` for N > 0
/// with non-Copy types. We bypass with a repetition macro.
macro_rules! canary_table_init {
    () => {
        CanaryTable {
            slots: {
                // SAFETY: CanarySlot #[repr(C)] without invalid padding; zeroes = CANARY_UNINIT initial.
                unsafe { core::mem::transmute::<[u8; MAX_CPUS * 64], [CanarySlot; MAX_CPUS]>([0u8; MAX_CPUS * 64]) }
            }
        }
    }
}

static CANARY_TABLE: CanaryTable = canary_table_init!();

// FIX-CANARY-01: compile-time assertions — detect any divergence
// if MAX_CPUS or CanarySlot change.
const _: () = assert!(
    core::mem::size_of::<CanarySlot>() == 64,
    "FIX-CANARY-01: CanarySlot doit faire exactement 64 octets (cache line)"
);
const _: () = assert!(
    core::mem::size_of::<CanaryTable>() == MAX_CPUS * 64,
    "FIX-CANARY-01: CanaryTable doit faire MAX_CPUS * 64 octets"
);

// ─────────────────────────────────────────────────────────────────────────────
// Pseudo-random generation via RDTSC
// ─────────────────────────────────────────────────────────────────────────────

/// Reads TSC via `rdtsc`.
/// # Safety: CPL 0 (or user RDTSC if CR4.TSD=0).
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Generates a pseudo-random canary value for `cpu_id`.
///
/// Mixes TSC + cpu_id + fixed constant to break predictions.
/// Not true randomness, but sufficient against blind stack
/// corruption attacks.
///
/// # Safety: CPL 0.
unsafe fn generate_canary(cpu_id: u32) -> u64 {
    let tsc = rdtsc();
    // Splitmix64-like: mix TSC with Fibonacci constant.
    let mut v = tsc ^ (cpu_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^ (v >> 31)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes canary for `cpu_id`. Must be called once per CPU
/// (BSP from `init()`, APs from their entry point).
///
/// # Safety
/// CPL 0 exclusive for this `cpu_id`.
pub unsafe fn init_cpu_canary(cpu_id: u32) {
    if cpu_id as usize >= MAX_CPUS {
        return;
    }
    let slot = &CANARY_TABLE.slots[cpu_id as usize];
    let val = generate_canary(cpu_id);
    slot.value.store(val, Ordering::Release);
    slot.generation.fetch_add(1, Ordering::Release);
    slot.initialized.store(true, Ordering::Release);
    CANARY_STATS.init_count.fetch_add(1, Ordering::Relaxed);
}

/// Returns current canary for `cpu_id`, or `CANARY_UNINIT` if uninitialized.
#[inline]
pub fn cpu_canary(cpu_id: u32) -> u64 {
    if cpu_id as usize >= MAX_CPUS {
        return CANARY_UNINIT;
    }
    CANARY_TABLE.slots[cpu_id as usize]
        .value
        .load(Ordering::Acquire)
}

/// Generates a thread canary = cpu_canary XOR tid.
/// Place this canary in thread's TCB upon creation.
#[inline]
pub fn thread_canary(cpu_id: u32, tid: u64) -> u64 {
    cpu_canary(cpu_id) ^ tid.wrapping_mul(0x517C_C1B7_2722_0A95)
}

/// Verifies thread canary `actual` against `expected`.
/// Returns `true` if OK, `false` on violation.
#[inline]
pub fn verify_thread_canary(expected: u64, actual: u64) -> bool {
    if expected == actual {
        CANARY_STATS.check_ok.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        canary_violation_handler(expected, actual);
    }
}

/// Called upon canary violation — logs + increments counter.
/// Caller must trigger a kernel panic.
pub fn canary_violation_handler(expected: u64, actual: u64) -> ! {
    CANARY_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = (expected, actual);
    panic!(
        "STACK CANARY VIOLATION: expected={:#018x} actual={:#018x}",
        expected, actual
    );
}

/// Performs canary rotation for all initialized CPUs.
/// Uses: periodic rotation, incident response, proactive hardening.
///
/// # Safety
/// CPL 0; must be called on BSP with global quiescence.
pub unsafe fn rotate_all_canaries() {
    for cpu_id in 0..MAX_CPUS as u32 {
        let slot = &CANARY_TABLE.slots[cpu_id as usize];
        if slot.initialized.load(Ordering::Acquire) {
            let val = generate_canary(cpu_id);
            slot.value.store(val, Ordering::Release);
            slot.generation.fetch_add(1, Ordering::Release);
            CANARY_STATS.rotate_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Initialization of canary subsystem for BSP (cpu_id = 0).
///
/// # Safety
/// CPL 0.
pub unsafe fn init() {
    init_cpu_canary(0);
}
