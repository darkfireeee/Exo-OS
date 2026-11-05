// kernel/src/memory/protection/smep.rs
//
// SMEP — Supervisor Mode Execution Prevention (Intel + AMD).
//
// The CPU refuses execution of code in user space (low canonical addresses)
// when CPL is 0 and CR4.SMEP = 1.
//
// References:
//   Intel SDM Vol.3A § 4.6.2 — "Supervisor-Mode Execution Prevention"
//   AMD APM Vol.2 § 5.16 — "Supervisor Mode Execution Prevention"
//
// Layer 0: no dependency on scheduler/process/ipc/fs.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// CR4 Bits
// ─────────────────────────────────────────────────────────────────────────────

/// CR4 bit 20 — SMEP.
pub const CR4_SMEP_BIT: u64 = 1 << 20;

// ─────────────────────────────────────────────────────────────────────────────
// SMEP Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct SmepStats {
    /// Number of SMEP activations.
    pub enable_count: AtomicU64,
    /// Number of temporary deactivations (legitimate, e.g. for user copy).
    pub disable_count: AtomicU64,
    /// Violations detected by fault handler.
    pub violation_count: AtomicU64,
    /// Redundant calls to `enable_smep` when already active.
    pub redundant_enable: AtomicU64,
}

impl SmepStats {
    const fn new() -> Self {
        Self {
            enable_count: AtomicU64::new(0),
            disable_count: AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            redundant_enable: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for SmepStats {}
pub static SMEP_STATS: SmepStats = SmepStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

static SMEP_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Read / write CR4
// ─────────────────────────────────────────────────────────────────────────────

/// Reads CR4.
///
/// # Safety
/// CPL 0.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {v}, cr4",
        v = out(reg) val,
        options(nostack, nomem, preserves_flags),
    );
    val
}

/// Writes CR4.
///
/// # Safety
/// CPL 0. An incorrect value can make the system unstable.
#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!(
        "mov cr4, {v}",
        v = in(reg) val,
        options(nostack, nomem),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Tests SMEP support via CPUID (leaf 7, subleaf 0, EBX bit 7).
#[inline]
pub fn smep_supported() -> bool {
    let ebx_r: u64;
    // SAFETY: CPUID available on x86_64; xchg preserves rbx reserved by LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            tmp = inout(reg) 0u64 => ebx_r,
            options(nostack, nomem),
        );
    }
    let ebx = ebx_r as u32;
    ebx & (1 << 7) != 0
}

/// Enables SMEP on current CPU (CR4.SMEP = 1).
/// Idempotent.
///
/// # Safety
/// CPL 0.
pub unsafe fn enable_smep() {
    if !smep_supported() {
        return;
    }
    let cr4 = read_cr4();
    if cr4 & CR4_SMEP_BIT != 0 {
        SMEP_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        return;
    }
    write_cr4(cr4 | CR4_SMEP_BIT);
    SMEP_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
    SMEP_ACTIVE.store(true, Ordering::Release);
}

/// Temporarily disables SMEP — DANGER, very restricted usage.
///
/// Returns `true` if SMEP was active (-> caller must call `restore_smep`).
/// Legitimate usage: kernel->user copy in pre-SMAP world, debug path only.
///
/// # Safety
/// CPL 0. The deactivation window must be as short as possible.
pub unsafe fn disable_smep() -> bool {
    let cr4 = read_cr4();
    if cr4 & CR4_SMEP_BIT == 0 {
        return false;
    }
    write_cr4(cr4 & !CR4_SMEP_BIT);
    SMEP_STATS.disable_count.fetch_add(1, Ordering::Relaxed);
    SMEP_ACTIVE.store(false, Ordering::Release);
    true
}

/// Re-enables SMEP if `was_active` is `true`.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn restore_smep(was_active: bool) {
    if was_active {
        enable_smep();
    }
}

/// Returns `true` if SMEP is currently active on this CPU.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn smep_active() -> bool {
    read_cr4() & CR4_SMEP_BIT != 0
}

/// Handles an SMEP violation detected by the fault handler.
/// Increments `violation_count` and returns `false` (non-recoverable).
#[inline]
pub fn smep_handle_violation(fault_ip: u64) -> bool {
    SMEP_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = fault_ip;
    false
}

/// RAII Guard: disables SMEP upon construction, restores it upon drop.
///
/// `SmepGuard::new()` returns `None` if SMEP was not active.
///
/// # Safety
/// Caller guarantees that user-code execution is intentional.
pub struct SmepGuard {
    was_active: bool,
}

impl SmepGuard {
    /// # Safety
    /// CPL 0.
    pub unsafe fn new() -> Self {
        let was_active = disable_smep();
        SmepGuard { was_active }
    }
}

impl Drop for SmepGuard {
    fn drop(&mut self) {
        // SAFETY: restore_smep restores CR4.SMEP; was_active comes from smep_enable().
        unsafe { restore_smep(self.was_active) };
    }
}

/// SMEP subsystem initialization (called on BSP + each AP).
///
/// # Safety
/// CPL 0.
pub unsafe fn init() {
    enable_smep();
}
