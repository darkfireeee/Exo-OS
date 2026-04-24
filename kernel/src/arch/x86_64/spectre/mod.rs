//! # arch/x86_64/spectre — Mitigations CPU (Spectre/Meltdown/MDS)
//!
//! Regroupe toutes les mitigations contre les vulnérabilités microarchitecturales :
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

/// Applique toutes les mitigations disponibles sur le CPU courant
///
/// Appelé lors de l'init de chaque CPU (BSP et APs).
pub fn apply_mitigations_bsp() {
    let feat = features::cpu_features();

    // KPTI si Meltdown possible et pas RDCL_NO
    if !feat.rdcl_no() {
        kpti::init_kpti();
    }

    // IBRS/STIBP si disponibles
    if feat.has_spec_ctrl() {
        ibrs::init_ibrs();
    }

    // SSBD si disponible
    if feat.has_ssbd() {
        ssbd::init_ssbd();
    }
}

/// Applique les mitigations sur un AP (après avoir chargé le contexte CPU)
pub fn apply_mitigations_ap() {
    apply_mitigations_bsp();
}
