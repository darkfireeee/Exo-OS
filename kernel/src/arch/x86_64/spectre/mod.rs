//! # arch/x86_64/spectre — CPU Mitigations (Spectre/Meltdown/MDS)
//!
//! Collects all mitigations against microarchitectural vulnerabilities:
//! - `kpti`       : Kernel Page-Table Isolation (Meltdown)
//! - `retpoline`  : Retpoline (Spectre variant 2)
//! - `ssbd`       : Speculative Store Bypass Disable (Spectre variant 4)
//! - `ibrs`       : Indirect Branch Restricted Speculation + STIBP + IBPB

pub mod ibrs;
pub mod kpti;
pub mod retpoline;
pub mod ssbd;

pub use ibrs::{apply_ibrs, apply_stibp, flush_ibpb, ibrs_enabled, stibp_enabled};
pub use kpti::{kpti_enabled, kpti_switch_to_kernel, kpti_switch_to_user};
pub use ssbd::{apply_ssbd_for_thread, ssbd_enabled};

use super::cpu::features;

/// Applies all available mitigations on the current CPU
///
/// Called during initialization of each CPU (BSP and APs).
pub fn apply_mitigations_bsp() {
    let feat = features::cpu_features();

    // KPTI if Meltdown possible and not RDCL_NO
    if !feat.rdcl_no() {
        kpti::init_kpti();
    }

    // IBRS/STIBP if available
    if feat.has_spec_ctrl() {
        ibrs::init_ibrs();
    }

    // SSBD if available
    if feat.has_ssbd() {
        ssbd::init_ssbd();
    }
}

/// Applies mitigations on an AP (after loading CPU context)
pub fn apply_mitigations_ap() {
    apply_mitigations_bsp();
}
