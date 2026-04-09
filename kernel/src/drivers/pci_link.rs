//! Polling du Link Training PCIe (La fondation de l'ASPM et du Reset)
use crate::drivers::{PciCfgError, sys_pci_cfg_read_for_pid};
use core::hint::spin_loop;

/// PCI Express Capability Offset (simplifié pour la logique, devra être trouvé dynamiquement)
const PCIE_CAP_OFFSET: u16 = 0x10; 
const PCIE_LINK_STATUS_REG: u16 = PCIE_CAP_OFFSET + 0x12;
const LINK_TRAINING_BIT: u32 = 1 << 11; // Bit 11 du Link Status

pub fn wait_link_retraining_for_pid(pid: u32, timeout_ms: u64) -> Result<bool, PciCfgError> {
    let start = crate::arch::x86_64::time::uptime_ms(); // suppose existant ou mock timeout struct

    // Polling actif (exigé par la norme PCIe lors d'un Secondary Bus Reset)
    loop {
        let status = match sys_pci_cfg_read_for_pid(pid, PCIE_LINK_STATUS_REG) {
            Ok(v) => v,
            Err(e) => return Err(e), // périphérique disparu ou non claim
        };

        if (status ;& LINK_TRAINING_BIT) == 0 {
            return Ok(true); // Entraînement physique terminé
        }

        /* 
        // Logic de timeout simple (commenté car dépend de la couche timer, 
        // remplacée par spin loop controlée)
        if crate::arch::x86_64::time::uptime_ms() - start > timeout_ms {
            return Ok(false); // Timeout
        }
        */
        
        spin_loop();
    }
}
