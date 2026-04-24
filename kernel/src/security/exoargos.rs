// kernel/src/security/exoargos.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoArgos — PMC Minimal Monitoring (ExoShield v1.0 Module 8)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoArgos performs minimal Performance Monitoring Counter (PMC) monitoring
// to detect anomalous behaviours (side-channel attacks, exfiltration,
// deception).
//
// Architecture :
//   • Hook into context_switch() for PMU snapshots via pmc_snapshot(tcb)
//   • Read 5 MSRs :
//       - IA32_FIXED_CTR0  (0x309) : Instructions retired
//       - IA32_FIXED_CTR1  (0x30A) : Core cycles unhalted
//       - IA32_PERFEVTSEL0 (0x186) + IA32_PMC0 (0xC1) : L3_MISS (0x20B1)
//       - IA32_PERFEVTSEL1 (0x187) + IA32_PMC1 (0xC2) : BR_MISP_RETIRED (0xC5)
//       - TSC via RDTSCP
//   • Integer-only scoring (NO f64) — fixed-point arithmetic with scale 10000
//   • DECEPTION_THRESHOLD = 3500 (0.35 in fixed-point)
//   • PmcSnapshot struct is exactly 64 bytes (matches SSR per-CPU PMC area)
//     Layout: inst_retired(8) + clk_unhalted(8) + l3_miss(8) + br_mispred(8)
//             + tsc(8) + reserved(24) = 64 bytes
//   • compute_discordance(oracle: u32, pmc: u32) -> u32
//       Computes |oracle − pmc| (absolute difference)
//   • check_anomaly() -> bool
//       Compares current snapshot against baseline, returns true if above
//       DECEPTION_THRESHOLD
//   • init_pmu() — program the PMU MSRs at boot
//   • Snapshots stored in SSR per-CPU area + static baseline
//
// ISR-SAFE where needed :
//   • No allocation
//   • No blocking locks (atomics + direct access)
//
// REFERENCES :
//   ExoShield_v1_Production.md — MODULE 8 : ExoArgos
//   Intel SDM Vol.3 §18 (Performance Monitoring)
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::cpu::msr;
use crate::arch::x86_64::cpu::tsc;
use crate::scheduler::core::task::ThreadControlBlock;

// ─────────────────────────────────────────────────────────────────────────────
// PMC MSR constants
// ─────────────────────────────────────────────────────────────────────────────

/// IA32_FIXED_CTR0 — Instructions retired (fixed counter 0).
const MSR_IA32_FIXED_CTR0: u32 = 0x0000_0309;
/// IA32_FIXED_CTR1 — Core cycles unhalted (fixed counter 1).
const MSR_IA32_FIXED_CTR1: u32 = 0x0000_030A;

/// IA32_PMC0 — Programmable counter 0.
const MSR_IA32_PMC0: u32 = 0x0000_00C1;
/// IA32_PMC1 — Programmable counter 1.
const MSR_IA32_PMC1: u32 = 0x0000_00C2;

/// IA32_PERFEVTSEL0 — PMU event selector 0.
const MSR_IA32_PERFEVTSEL0: u32 = 0x0000_0186;
/// IA32_PERFEVTSEL1 — PMU event selector 1.
const MSR_IA32_PERFEVTSEL1: u32 = 0x0000_0187;

/// IA32_FIXED_CTR_CTRL — Fixed counter control.
const MSR_IA32_FIXED_CTR_CTRL: u32 = 0x0000_038D;
/// IA32_PERF_GLOBAL_CTRL — Global PMU enable.
const MSR_IA32_PERF_GLOBAL_CTRL: u32 = 0x0000_038F;
/// IA32_PERF_GLOBAL_OVF_CTRL — Global overflow control.
const MSR_IA32_PERF_GLOBAL_OVF_CTRL: u32 = 0x0000_0390;

// ─────────────────────────────────────────────────────────────────────────────
// PMU events (Event Select + Unit Mask)
// ─────────────────────────────────────────────────────────────────────────────

/// MEM_LOAD_RETIRED.L3_MISS : Event=0xD1, UMask=0x01
/// Counts retired load instructions that missed the L3 cache.
const EVENT_MEM_LOAD_RETIRED_L3_MISS: u64 = (0x01 << 8) | 0xD1;

/// BR_MISP_RETIRED.ALL_BRANCHES : Event=0xC5, UMask=0x00
/// Counts retired mispredicted branch instructions.
const EVENT_BR_MISP_RETIRED_ALL: u64 = (0x00 << 8) | 0xC5;

// ─────────────────────────────────────────────────────────────────────────────
// PMU control bits
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 0: Enable counter.
const PERFEVTSEL_EN: u64 = 1 << 0;
/// Bit 1: User mode counting.
const PERFEVTSEL_USR: u64 = 1 << 1;
/// Bit 2: OS/kernel mode counting.
const PERFEVTSEL_OS: u64 = 1 << 2;

/// Bits [1:0] shifted by 4 for Fixed Counter 1 enable.
const FIXED_CTR1_EN_SHIFT: u64 = 4;
/// Mask to enable a fixed counter in OS + USR modes (bits [1:0] = 0x03).
const FIXED_CTR_EN_OS_USR: u64 = 0x03;

/// Bit 0 of PERF_GLOBAL_CTRL: Enable PMC0.
const GLOBAL_PMC0_EN: u64 = 1 << 0;
/// Bit 1 of PERF_GLOBAL_CTRL: Enable PMC1.
const GLOBAL_PMC1_EN: u64 = 1 << 1;
/// Bit 32 of PERF_GLOBAL_CTRL: Enable Fixed Counter 0.
const GLOBAL_FIXED_CTR0_EN: u64 = 1 << 32;
/// Bit 33 of PERF_GLOBAL_CTRL: Enable Fixed Counter 1.
const GLOBAL_FIXED_CTR1_EN: u64 = 1 << 33;

// ─────────────────────────────────────────────────────────────────────────────
// Deception threshold
// ─────────────────────────────────────────────────────────────────────────────

/// Fixed-point scale factor for discordance computation (4 decimal places).
const FP_SCALE: u64 = 10000;

/// Deception threshold in fixed-point (0.35 × 10000 = 3500).
/// If the snapshot-level discordance exceeds this threshold, an anomaly
/// is flagged.
pub const DECEPTION_THRESHOLD: u32 = 3500;

/// Number of PMC metrics used in the snapshot-level discordance.
const NUM_METRICS: u64 = 5;

// ─────────────────────────────────────────────────────────────────────────────
// PmcSnapshot — 64-byte structure matching SSR per-CPU PMC area
// ─────────────────────────────────────────────────────────────────────────────

/// PMC counter snapshot for one CPU core.
///
/// Size: 5 × 8 + 3 × 8 = 64 bytes (matches SSR_PMC_SNAPSHOT_SIZE).
///
/// Layout:
///   [0]   inst_retired  : u64   — IA32_FIXED_CTR0
///   [8]   clk_unhalted  : u64   — IA32_FIXED_CTR1
///   [16]  l3_miss       : u64   — IA32_PMC0 (MEM_LOAD_RETIRED.L3_MISS)
///   [24]  br_mispredict : u64   — IA32_PMC1 (BR_MISP_RETIRED.ALL_BRANCHES)
///   [32]  tsc           : u64   — RDTSCP
///   [40]  _reserved     : [u64; 3] — alignment to 64 bytes
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PmcSnapshot {
    /// IA32_FIXED_CTR0 — Instructions retired.
    pub inst_retired: u64,
    /// IA32_FIXED_CTR1 — Core cycles unhalted.
    pub clk_unhalted: u64,
    /// IA32_PMC0 — MEM_LOAD_RETIRED.L3_MISS.
    pub l3_miss: u64,
    /// IA32_PMC1 — BR_MISP_RETIRED.ALL_BRANCHES.
    pub br_mispredict: u64,
    /// TSC — Time Stamp Counter.
    pub tsc: u64,
    /// Reserved (SSR 64-byte alignment).
    pub _reserved: [u64; 3],
}

// Static assertion: PmcSnapshot is exactly 64 bytes
const _: () = assert!(
    core::mem::size_of::<PmcSnapshot>() == 64,
    "PmcSnapshot must be exactly 64 bytes (SSR_PMC_SNAPSHOT_SIZE)"
);

// ─────────────────────────────────────────────────────────────────────────────
// Global ExoArgos state
// ─────────────────────────────────────────────────────────────────────────────

/// PMU initialized and functional.
static PMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Baseline snapshot (oracle) — one per core max.
/// For minimal monitoring, a single global baseline is stored.
static BASELINE_SNAPSHOT: PmcSnapshotInner = PmcSnapshotInner::new();

/// Whether the baseline has been established (at least one snapshot recorded).
static BASELINE_ESTABLISHED: AtomicBool = AtomicBool::new(false);

/// Total anomalies detected.
static ANOMALY_COUNT: AtomicU64 = AtomicU64::new(0);

/// Total snapshots captured.
static SNAPSHOT_COUNT: AtomicU64 = AtomicU64::new(0);
const PMC_TCB_MISMATCH_TAG: u64 = 0x504d_435f_4d49_534d;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotSubjectError {
    CurrentThreadUnavailable,
    TcbMismatch,
}

#[inline(always)]
fn validate_snapshot_subject_ptrs(
    current: *const ThreadControlBlock,
    candidate: *const ThreadControlBlock,
) -> Result<(), SnapshotSubjectError> {
    if current.is_null() {
        return Err(SnapshotSubjectError::CurrentThreadUnavailable);
    }
    if current != candidate {
        return Err(SnapshotSubjectError::TcbMismatch);
    }
    Ok(())
}

/// Inner storage for the baseline snapshot using individual atomics
/// (ISR-safe: no locks, no allocation).
struct PmcSnapshotInner {
    inst_retired: AtomicU64,
    clk_unhalted: AtomicU64,
    l3_miss: AtomicU64,
    br_mispredict: AtomicU64,
    tsc: AtomicU64,
}

impl PmcSnapshotInner {
    const fn new() -> Self {
        Self {
            inst_retired: AtomicU64::new(0),
            clk_unhalted: AtomicU64::new(0),
            l3_miss: AtomicU64::new(0),
            br_mispredict: AtomicU64::new(0),
            tsc: AtomicU64::new(0),
        }
    }

    fn store(&self, snap: &PmcSnapshot) {
        self.inst_retired
            .store(snap.inst_retired, Ordering::Release);
        self.clk_unhalted
            .store(snap.clk_unhalted, Ordering::Release);
        self.l3_miss.store(snap.l3_miss, Ordering::Release);
        self.br_mispredict
            .store(snap.br_mispredict, Ordering::Release);
        self.tsc.store(snap.tsc, Ordering::Release);
    }

    fn load(&self) -> PmcSnapshot {
        PmcSnapshot {
            inst_retired: self.inst_retired.load(Ordering::Acquire),
            clk_unhalted: self.clk_unhalted.load(Ordering::Acquire),
            l3_miss: self.l3_miss.load(Ordering::Acquire),
            br_mispredict: self.br_mispredict.load(Ordering::Acquire),
            tsc: self.tsc.load(Ordering::Acquire),
            _reserved: [0u64; 3],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PMU initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the PMU counters for ExoArgos monitoring.
///
/// Programs:
/// 1. Fixed Counters 0 and 1 (instructions retired, core cycles unhalted)
/// 2. Programmable Counter 0 → MEM_LOAD_RETIRED.L3_MISS
/// 3. Programmable Counter 1 → BR_MISP_RETIRED.ALL_BRANCHES
/// 4. Enables the PMU globally
///
/// # Safety
/// Must be called from Ring 0, exactly once at boot.
pub unsafe fn exoargos_init() {
    // 1. Check PMU support via CPUID leaf 0xA
    let eax_a = core::arch::x86_64::__cpuid(0x0A).eax;

    // Version ID (bits [7:0] of EAX) : 0 = no PMU
    let pmu_version = eax_a & 0xFF;
    if pmu_version == 0 {
        return; // PMU not supported
    }

    // 2. Program the event selectors
    // PERFEVTSEL0 = MEM_LOAD_RETIRED.L3_MISS | EN | OS | USR
    unsafe {
        msr::write_msr(
            MSR_IA32_PERFEVTSEL0,
            EVENT_MEM_LOAD_RETIRED_L3_MISS | PERFEVTSEL_EN | PERFEVTSEL_OS | PERFEVTSEL_USR,
        );

        // PERFEVTSEL1 = BR_MISP_RETIRED.ALL_BRANCHES | EN | OS | USR
        msr::write_msr(
            MSR_IA32_PERFEVTSEL1,
            EVENT_BR_MISP_RETIRED_ALL | PERFEVTSEL_EN | PERFEVTSEL_OS | PERFEVTSEL_USR,
        );

        // 3. Enable fixed counters (CTR0 + CTR1 in OS + USR modes)
        // FIXED_CTR_CTRL = [CTR1: OS+USR] | [CTR0: OS+USR]
        let fixed_ctrl = (FIXED_CTR_EN_OS_USR << FIXED_CTR1_EN_SHIFT) | FIXED_CTR_EN_OS_USR;
        msr::write_msr(MSR_IA32_FIXED_CTR_CTRL, fixed_ctrl as u64);

        // 4. Clear overflow flags
        msr::write_msr(
            MSR_IA32_PERF_GLOBAL_OVF_CTRL,
            GLOBAL_FIXED_CTR0_EN | GLOBAL_FIXED_CTR1_EN | GLOBAL_PMC0_EN | GLOBAL_PMC1_EN,
        );

        // 5. Enable the PMU globally
        msr::write_msr(
            MSR_IA32_PERF_GLOBAL_CTRL,
            GLOBAL_FIXED_CTR0_EN | GLOBAL_FIXED_CTR1_EN | GLOBAL_PMC0_EN | GLOBAL_PMC1_EN,
        );
    }

    PMU_INITIALIZED.store(true, Ordering::Release);
}

/// Alias for `exoargos_init()` — programs the PMU MSRs at boot.
///
/// # Safety
/// Same requirements as `exoargos_init()`: Ring 0, called once.
#[inline(always)]
pub unsafe fn init_pmu() {
    exoargos_init()
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Captures a PMC snapshot from the current CPU.
///
/// Reads the 5 MSRs and returns a PmcSnapshot. If the PMU is not
/// initialized, returns a zeroed snapshot.
///
/// This function is designed to be called from context_switch()
/// to capture the outgoing thread's PMU state. The ThreadControlBlock
/// reference is provided for future per-thread PMC tracking.
pub fn pmc_snapshot(tcb: &ThreadControlBlock) -> PmcSnapshot {
    if !PMU_INITIALIZED.load(Ordering::Acquire) {
        return PmcSnapshot::default();
    }

    let current = crate::scheduler::core::switch::current_thread_raw() as *const ThreadControlBlock;
    if let Err(err) = validate_snapshot_subject_ptrs(current, tcb as *const ThreadControlBlock) {
        if matches!(err, SnapshotSubjectError::TcbMismatch) {
            crate::security::exoledger::exo_ledger_append(
                crate::security::exoledger::ActionTag::Custom {
                    tag: PMC_TCB_MISMATCH_TAG,
                    data: tcb.pid.0 as u64,
                },
            );
        }
        return PmcSnapshot::default();
    }

    let inst_retired = unsafe { msr::read_msr(MSR_IA32_FIXED_CTR0) };
    let clk_unhalted = unsafe { msr::read_msr(MSR_IA32_FIXED_CTR1) };
    let l3_miss = unsafe { msr::read_msr(MSR_IA32_PMC0) };
    let br_mispredict = unsafe { msr::read_msr(MSR_IA32_PMC1) };
    let tsc_val = tsc::read_tsc();

    let snap = PmcSnapshot {
        inst_retired,
        clk_unhalted,
        l3_miss,
        br_mispredict,
        tsc: tsc_val,
        _reserved: [0u64; 3],
    };

    SNAPSHOT_COUNT.fetch_add(1, Ordering::Relaxed);

    // Write the snapshot to the SSR per-CPU area
    write_snapshot_to_ssr(&snap);

    snap
}

/// Writes a PMC snapshot to the SSR per-CPU area.
///
/// The offset is computed from the logical CPU ID (TSC_AUX or APIC ID).
fn write_snapshot_to_ssr(snap: &PmcSnapshot) {
    // Determine current CPU ID
    let cpu_id = if crate::arch::x86_64::cpu::features::cpu_features_or_none()
        .map_or(false, |features| features.has_rdtscp())
    {
        let (_, aux) = unsafe { msr::rdtscp() };
        aux
    } else {
        0 // Fallback: CPU 0
    };

    let offset = crate::exophoenix::ssr::pmc_snapshot_offset(cpu_id as usize);
    let ssr_base = crate::memory::phys_to_virt(crate::memory::core::PhysAddr::new(
        crate::exophoenix::ssr::SSR_BASE,
    ))
    .as_u64() as usize;

    // SAFETY: The snapshot is exactly 64 bytes (SSR_PMC_SNAPSHOT_SIZE)
    // and the offset has been validated by SSR static assertions.
    unsafe {
        let dst = (ssr_base + offset) as *mut u8;
        core::ptr::copy_nonoverlapping(
            snap as *const PmcSnapshot as *const u8,
            dst,
            core::mem::size_of::<PmcSnapshot>(),
        );
    }
}

/// Computes the discordance between two individual counter values.
///
/// Returns the absolute difference |oracle − pmc| as a u32.
/// This is the primitive building block for anomaly scoring:
/// higher-level logic normalizes and aggregates per-metric discordances
/// into a fixed-point composite score.
///
/// # Arguments
/// * `oracle` — Baseline (expected) counter value, truncated to u32.
/// * `pmc`    — Observed counter value, truncated to u32.
///
/// # Returns
/// `|oracle − pmc|` as u32. Saturates on overflow (impossible for u32 diff).
#[inline(always)]
pub fn compute_discordance(oracle: u32, pmc: u32) -> u32 {
    if oracle >= pmc {
        oracle - pmc
    } else {
        pmc - oracle
    }
}

/// Computes the composite fixed-point discordance between two full snapshots.
///
/// For each of the 5 metrics, computes the normalized per-metric discordance:
///   d_i = |oracle_i − pmc_i| × FP_SCALE / max(oracle_i, 1)
///
/// The composite discordance is the mean across all 5 metrics:
///   D = Σ d_i / NUM_METRICS
///
/// All arithmetic is integer-only (no f64). Uses u128 intermediates
/// to prevent overflow.
///
/// Returns the discordance as u32 in fixed-point (scale 10000).
/// Example: 3500 = 0.35 composite discordance.
fn compute_snapshot_discordance(oracle: &PmcSnapshot, pmc: &PmcSnapshot) -> u32 {
    let metrics: [(u64, u64); NUM_METRICS as usize] = [
        (oracle.inst_retired, pmc.inst_retired),
        (oracle.clk_unhalted, pmc.clk_unhalted),
        (oracle.l3_miss, pmc.l3_miss),
        (oracle.br_mispredict, pmc.br_mispredict),
        (oracle.tsc, pmc.tsc),
    ];

    let mut total: u64 = 0;
    for (o, p) in &metrics {
        let diff = if o > p { o - p } else { p - o };
        let divisor = (*o).max(1);
        // Fixed-point division: diff × FP_SCALE / divisor
        // Use u128 to prevent overflow
        let scaled = ((diff as u128) * (FP_SCALE as u128) / (divisor as u128)) as u64;
        total = total.saturating_add(scaled);
    }

    // Average across all metrics
    let avg = total / NUM_METRICS;

    avg as u32
}

/// Checks whether the current behaviour is anomalous relative to the baseline.
///
/// Captures a PMC snapshot, compares it to the stored baseline, and returns
/// `true` if the composite discordance exceeds DECEPTION_THRESHOLD.
///
/// If the baseline has not yet been established, the first snapshot becomes
/// the baseline and `false` is returned.
///
/// Uses integer-only fixed-point arithmetic (scale 10000, threshold 3500).
pub fn check_anomaly() -> bool {
    // Capture a snapshot without a TCB reference (system-wide check)
    if !PMU_INITIALIZED.load(Ordering::Acquire) {
        return false;
    }

    let inst_retired = unsafe { msr::read_msr(MSR_IA32_FIXED_CTR0) };
    let clk_unhalted = unsafe { msr::read_msr(MSR_IA32_FIXED_CTR1) };
    let l3_miss = unsafe { msr::read_msr(MSR_IA32_PMC0) };
    let br_mispredict = unsafe { msr::read_msr(MSR_IA32_PMC1) };
    let tsc_val = tsc::read_tsc();

    let current = PmcSnapshot {
        inst_retired,
        clk_unhalted,
        l3_miss,
        br_mispredict,
        tsc: tsc_val,
        _reserved: [0u64; 3],
    };

    SNAPSHOT_COUNT.fetch_add(1, Ordering::Relaxed);

    // If the baseline is not yet established, store this snapshot as baseline
    if !BASELINE_ESTABLISHED.load(Ordering::Acquire) {
        BASELINE_SNAPSHOT.store(&current);
        BASELINE_ESTABLISHED.store(true, Ordering::Release);
        return false;
    }

    let oracle = BASELINE_SNAPSHOT.load();
    let discordance = compute_snapshot_discordance(&oracle, &current);

    if discordance > DECEPTION_THRESHOLD {
        ANOMALY_COUNT.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Updates the baseline with the given snapshot.
///
/// Used after a verified-safe context to adjust the baseline to
/// legitimate workload changes.
pub fn update_baseline(snap: &PmcSnapshot) {
    BASELINE_SNAPSHOT.store(snap);
    BASELINE_ESTABLISHED.store(true, Ordering::Release);
}

/// Returns the current baseline snapshot.
pub fn get_baseline() -> PmcSnapshot {
    BASELINE_SNAPSHOT.load()
}

/// Returns whether the baseline has been established.
#[inline(always)]
pub fn baseline_established() -> bool {
    BASELINE_ESTABLISHED.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of ExoArgos statistics.
#[derive(Debug, Clone, Copy)]
pub struct ExoArgosStats {
    /// Total anomalies detected.
    pub anomaly_count: u64,
    /// Total snapshots captured.
    pub snapshot_count: u64,
    /// Whether the baseline has been established.
    pub baseline_established: bool,
}

/// Returns a snapshot of ExoArgos statistics.
pub fn exoargos_stats() -> ExoArgosStats {
    ExoArgosStats {
        anomaly_count: ANOMALY_COUNT.load(Ordering::Relaxed),
        snapshot_count: SNAPSHOT_COUNT.load(Ordering::Relaxed),
        baseline_established: BASELINE_ESTABLISHED.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_snapshot_subject_ptrs, SnapshotSubjectError};
    use crate::scheduler::core::task::{
        Priority, ProcessId, SchedPolicy, ThreadControlBlock, ThreadId,
    };

    fn make_tcb(id: u64, pid: u32) -> ThreadControlBlock {
        ThreadControlBlock::new(
            ThreadId(id),
            ProcessId(pid),
            SchedPolicy::Normal,
            Priority::NORMAL_DEFAULT,
            0,
            0x1000,
        )
    }

    #[test]
    fn test_validate_snapshot_subject_ptrs_rejects_mismatch() {
        let current = make_tcb(1, 7);
        let foreign = make_tcb(2, 8);
        assert_eq!(
            validate_snapshot_subject_ptrs(&current, &foreign),
            Err(SnapshotSubjectError::TcbMismatch)
        );
    }

    #[test]
    fn test_validate_snapshot_subject_ptrs_accepts_current_thread() {
        let current = make_tcb(3, 9);
        assert_eq!(validate_snapshot_subject_ptrs(&current, &current), Ok(()));
    }
}
