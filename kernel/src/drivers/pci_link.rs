//! # drivers/pci_link.rs
//!
//! Helpers GI-03 de lifecycle PCIe liés au link training.
//! Le chemin actif repose sur les accès config réels de `pci_cfg.rs`.

use super::{pci_cfg, PciCfgError};

/// Attend la fin du retraining du lien PCIe du bridge parent du device claimé.
///
/// La logique matérielle réelle vit dans `pci_cfg.rs` :
/// - lookup topologie parent bridge
/// - découverte capability PCIe
/// - polling Link Status avec fallback 250 ms si pas de bridge
pub fn wait_link_retraining_for_pid(pid: u32, timeout_ms: u64) -> Result<bool, PciCfgError> {
    pci_cfg::sys_wait_link_retraining_for_pid(pid, timeout_ms)
}
