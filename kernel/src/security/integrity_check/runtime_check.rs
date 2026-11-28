// kernel/src/security/integrity_check/runtime_check.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Runtime Integrity Check — Periodic verification of kernel sections
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • BLAKE3 hash of .text and .rodata sections calculated at boot
//   • Periodic verification from a kernel timer (or on-demand)
//   • In case of detected alteration → immediate kernel panic
//   • Instrumentation: check count, timestamp of last successful check
//
// RULE RUNTIME-01: Initial hashes are calculated BEFORE enabling IRQs.
// RULE RUNTIME-02: Detected alteration → IMMEDIATE kernel_panic(), no retry.
// RULE RUNTIME-03: .text/.rodata addresses originate from the linker script (externs).
// ═══════════════════════════════════════════════════════════════════════════════

use super::super::crypto::blake3::blake3_hash;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Linker script symbols (kernel/linker.ld)
// ─────────────────────────────────────────────────────────────────────────────

// SAFETY: These symbols are defined in kernel/linker.ld and are always
// present in a properly compiled kernel. Addresses are in the
// read-only code section — no concurrent access is possible
// since .text/.rodata are never written after loading.
extern "C" {
    static _text_start: u8;
    static _text_end: u8;
    static _rodata_start: u8;
    static _rodata_end: u8;
}

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum IntegrityError {
    /// .text section altered.
    TextSectionCorrupted,
    /// .rodata section altered.
    RodataSectionCorrupted,
    /// Stack guard section altered.
    StackGuardCorrupted,
    /// Not initialized.
    NotInitialized,
}

// ─────────────────────────────────────────────────────────────────────────────
// RuntimeIntegrityChecker — check state
// ─────────────────────────────────────────────────────────────────────────────

struct RuntimeIntegrityState {
    /// Reference hash of the .text section.
    text_hash: [u8; 32],
    /// Reference hash of the .rodata section.
    rodata_hash: [u8; 32],
    /// Initialized?
    initialized: bool,
    /// Number of successful checks.
    checks_ok: u64,
    /// Last TSC timestamp of successful check.
    last_ok_tsc: u64,
}

impl RuntimeIntegrityState {
    const fn new() -> Self {
        Self {
            text_hash: [0u8; 32],
            rodata_hash: [0u8; 32],
            initialized: false,
            checks_ok: 0,
            last_ok_tsc: 0,
        }
    }
}

static INTEGRITY_STATE: spin::Mutex<RuntimeIntegrityState> =
    spin::Mutex::new(RuntimeIntegrityState::new());
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static CHECKS_PERFORMED: AtomicU64 = AtomicU64::new(0);
static VIOLATIONS_DETECTED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Utility functions for kernel sections
// ─────────────────────────────────────────────────────────────────────────────

/// Returns slice of the .text section.
// SAFETY: Low-level operation validated — see block documentation.
unsafe fn text_section() -> &'static [u8] {
    // SAFETY: Symbols _text_start/_text_end are defined by the linker script.
    // The .text section is valid and correctly sized. Read in read-only mode.
    let start = &_text_start as *const u8;
    let end = &_text_end as *const u8;
    let len = end as usize - start as usize;
    core::slice::from_raw_parts(start, len)
}

/// Returns slice of the .rodata section.
// SAFETY: Low-level operation validated — see block documentation.
unsafe fn rodata_section() -> &'static [u8] {
    // SAFETY: Identical to text_section() — linker symbols, read-only.
    let start = &_rodata_start as *const u8;
    let end = &_rodata_end as *const u8;
    let len = end as usize - start as usize;
    core::slice::from_raw_parts(start, len)
}

/// Reads the current TSC.
#[cfg(target_arch = "x86_64")]
fn read_tsc() -> u64 {
    crate::arch::x86_64::cpu::tsc::read_tsc()
}

#[cfg(not(target_arch = "x86_64"))]
fn read_tsc() -> u64 {
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the kernel reference hashes.
///
/// **MUST be called before enabling IRQs** (RULE RUNTIME-01).
/// Must be called only once.
pub fn init_runtime_integrity() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return; // Protected double-init
    }
    let mut state = INTEGRITY_STATE.lock();

    // SAFETY: See text_section() / rodata_section() — valid linker symbols.
    let text_hash = unsafe { blake3_hash(text_section()) };
    let rodata_hash = unsafe { blake3_hash(rodata_section()) };

    state.text_hash = text_hash;
    state.rodata_hash = rodata_hash;
    state.initialized = true;
    state.last_ok_tsc = read_tsc();
}

/// Hashes a section in 256 KiB chunks by cooperatively yielding the CPU
/// between chunks if the scheduler is active, in order to never block
/// interactive threads during the hashing of the ~15 MiB `.text`/`.rodata`.
fn hash_section_cooperative(section: &[u8]) -> [u8; 32] {
    const CHUNK_SIZE: usize = 32 * 1024;
    let mut hasher = super::super::crypto::blake3::Blake3Hasher::new();
    for chunk in section.chunks(CHUNK_SIZE) {
        hasher.update(chunk);
        // SAFETY: cooperative_reschedule internally verifies TCB, IRQ, and preempt_count.
        unsafe {
            let _ = crate::scheduler::core::switch::cooperative_reschedule();
        }
    }
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

/// Performs an integrity check of the kernel sections.
///
/// Returns Err if alteration is detected.
/// **In production: call kernel_panic() on Err.**
pub fn check_kernel_integrity() -> Result<(), IntegrityError> {
    let (expected_text, expected_rodata) = {
        let state = INTEGRITY_STATE.lock();
        if !state.initialized {
            return Err(IntegrityError::NotInitialized);
        }
        (state.text_hash, state.rodata_hash)
    };

    // Calculate current hashes outside spinlock, in cooperative chunks
    // SAFETY: See text_section() / rodata_section().
    let current_text = unsafe { hash_section_cooperative(text_section()) };
    let current_rodata = unsafe { hash_section_cooperative(rodata_section()) };

    // Compare in constant time
    let mut text_diff = 0u8;
    let mut rodata_diff = 0u8;
    for i in 0..32 {
        text_diff |= current_text[i] ^ expected_text[i];
        rodata_diff |= current_rodata[i] ^ expected_rodata[i];
    }

    CHECKS_PERFORMED.fetch_add(1, Ordering::Relaxed);

    if text_diff != 0 {
        VIOLATIONS_DETECTED.fetch_add(1, Ordering::Relaxed);
        return Err(IntegrityError::TextSectionCorrupted);
    }
    if rodata_diff != 0 {
        VIOLATIONS_DETECTED.fetch_add(1, Ordering::Relaxed);
        return Err(IntegrityError::RodataSectionCorrupted);
    }

    // Update success timestamp
    let mut state = INTEGRITY_STATE.lock();
    state.checks_ok += 1;
    state.last_ok_tsc = read_tsc();
    Ok(())
}

/// Verifies integrity and panics if compromised.
pub fn assert_kernel_integrity() {
    if let Err(e) = check_kernel_integrity() {
        // In no_std kernel: panic with error message
        panic!("KERNEL INTEGRITY VIOLATION: {:?}", e);
    }
}

/// Active remediation policy: when `true`, a detected `.text`/`.rodata` corruption
/// triggers an `ExoLedger` P0 alert AND requests an ExoPhoenix Kernel B handoff via
/// `SSR_HANDOFF_FLAG` (FIX-18).
static ENFORCE_HANDOFF_ON_VIOLATION: AtomicBool = AtomicBool::new(true);

/// Returns the boot-time reference hashes `(text_hash, rodata_hash)` once initialized.
pub fn reference_hashes() -> Option<([u8; 32], [u8; 32])> {
    let state = INTEGRITY_STATE.lock();
    if !state.initialized {
        return None;
    }
    Some((state.text_hash, state.rodata_hash))
}

/// Enables or disables automatic ExoPhoenix handoff escalation on runtime integrity failure.
pub fn set_enforce_handoff_on_violation(enabled: bool) {
    ENFORCE_HANDOFF_ON_VIOLATION.store(enabled, Ordering::Release);
}

/// FIX-18: Records a kernel section integrity violation in `ExoLedger` P0 and escalates
/// to ExoPhoenix handoff (`SSR_HANDOFF_FLAG = 1`) so Kernel B heals the corrupted image
/// from the Golden RAM Mirror. Returns `true` if handoff escalation was triggered.
pub fn handle_integrity_violation(e: IntegrityError) -> bool {
    let code = match e {
        IntegrityError::TextSectionCorrupted => 1,
        IntegrityError::RodataSectionCorrupted => 2,
        IntegrityError::StackGuardCorrupted => 3,
        IntegrityError::NotInitialized => 0,
    };
    if code == 0 {
        return false;
    }
    // 0x494E_5445_4752_5459 = b"INTEGRTY" — custom integrity audit tag.
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::Custom {
            tag: 0x494E_5445_4752_5459,
            data: code,
        },
    );
    if ENFORCE_HANDOFF_ON_VIOLATION.load(Ordering::Acquire) {
        #[cfg(target_os = "none")]
        // SAFETY: SSR_HANDOFF_FLAG is a mapped Ring 0 shared handoff trigger cell.
        unsafe {
            crate::exophoenix::ssr::ssr_atomic(crate::exophoenix::ssr::SSR_HANDOFF_FLAG)
                .store(1, Ordering::Release);
        }
        return true;
    }
    false
}

/// Periodic kernel `.text`/`.rodata` integrity check (FIX-18).
///
/// Logs any alteration into `ExoLedger` P0, increments `VIOLATIONS_DETECTED`,
/// and escalates to ExoPhoenix Kernel B handoff (`SSR_HANDOFF_FLAG = 1`) for
/// self-healing via the Golden RAM Mirror.
pub fn security_periodic_check_observe() {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    if let Err(e) = check_kernel_integrity() {
        let _ = handle_integrity_violation(e);
    }
}

/// Returns true if the integrity system is initialized.
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

#[derive(Debug, Clone, Copy)]
pub struct IntegrityStats {
    pub checks_performed: u64,
    pub violations_detected: u64,
    pub last_ok_tsc: u64,
}

pub fn integrity_stats() -> IntegrityStats {
    let state = INTEGRITY_STATE.lock();
    IntegrityStats {
        checks_performed: CHECKS_PERFORMED.load(Ordering::Relaxed),
        violations_detected: VIOLATIONS_DETECTED.load(Ordering::Relaxed),
        last_ok_tsc: state.last_ok_tsc,
    }
}

#[cfg(test)]
mod fix18_runtime_integrity_tests {
    use super::*;

    #[test]
    fn var20_text_corruption_triggers_exoledger_and_handoff_escalation() {
        set_enforce_handoff_on_violation(true);
        assert!(handle_integrity_violation(
            IntegrityError::TextSectionCorrupted
        ));
        assert!(handle_integrity_violation(
            IntegrityError::RodataSectionCorrupted
        ));
        assert!(!handle_integrity_violation(IntegrityError::NotInitialized));
    }
}
