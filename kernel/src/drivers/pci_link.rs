//! # drivers/pci_link.rs
//!
//! PCIe lifecycle GI-03 helpers related to link training.
//! The active path relies on real config accesses in `pci_cfg.rs`.

use super::{pci_cfg, PciCfgError};

/// Waits for the completion of PCIe link retraining of the claimed device's parent bridge.
///
/// Actual hardware logic lives in `pci_cfg.rs`:
/// - parent bridge topology lookup
/// - PCIe capability discovery
/// - Link Status polling with 250 ms fallback if no bridge
pub fn wait_link_retraining_for_pid(pid: u32, timeout_ms: u64) -> Result<bool, PciCfgError> {
    pci_cfg::sys_wait_link_retraining_for_pid(pid, timeout_ms)
}
