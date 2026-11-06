// kernel/src/security/integrity_check/mod.rs
//
// Module integrity_check — Multi-level integrity verification
//
// Submodules:
//   • code_signing   — Ed25519 signature of kernel modules
//   • runtime_check  — Periodic BLAKE3 hashing of .text/.rodata
//   • secure_boot    — Chain of trust exo-boot → kernel

pub mod code_signing;
pub mod runtime_check;
pub mod secure_boot;

pub use code_signing::{
    code_sign_stats, register_loaded_module, verify_module_signature, CodeSignError, ModuleHeader,
};

pub use runtime_check::{
    assert_kernel_integrity, check_kernel_integrity, init_runtime_integrity, integrity_stats,
    security_periodic_check_observe, IntegrityError,
};

pub use secure_boot::{
    boot_nonce, check_chain_of_trust, extend_pcr, is_chain_verified, read_pcr, secureboot_stats,
    verify_boot_attestation, BootAttestation, SecureBootError,
};

/// Initializes the integrity subsystem.
///
/// Order: runtime_check must run BEFORE any other init
/// (the .text/.rodata sections must be intact).
pub fn integrity_init() {
    init_runtime_integrity();
}

/// Interval between two runtime integrity checks (60 s). The Blake3
/// hash of `.text/.rodata` is a heavy operation → low frequency
/// sufficient to detect persistent alteration without unnecessary load.
const INTEGRITY_CHECK_INTERVAL_NS: u64 = 60_000_000_000;

/// Loop of the integrity monitor kthread (TIER 2.1-a). Sleeps, then — once the
/// security boot is finished (`is_security_ready`, thus `.text/.rodata` stable) —
/// triggers a check in **observe mode** (log/ledger, never panics).
fn integrity_monitor_loop(_arg: usize) -> ! {
    loop {
        if !crate::scheduler::timer::sleep_ns(INTEGRITY_CHECK_INTERVAL_NS) {
            // Sleep unavailable (very early) → yield the CPU and retry.
            // SAFETY: cooperative_reschedule yields cleanly from a kthread.
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
            continue;
        }
        if crate::security::is_security_ready() {
            security_periodic_check_observe();
        }
    }
}

/// Starts the runtime integrity monitor kthread. Best-effort: if kthread creation
/// fails, boot continues (integrity remains checkable on-demand
/// via `check_kernel_integrity`). To be called once after `security_init`.
pub fn start_integrity_monitor() {
    use crate::process::lifecycle::create::{create_kthread, KthreadParams};
    use crate::scheduler::core::task::Priority;
    let _ = create_kthread(&KthreadParams {
        name: "integrity-mon",
        entry: integrity_monitor_loop,
        arg: 0,
        target_cpu: 0,
        priority: Priority::NORMAL_MIN,
    });
}
