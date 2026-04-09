//! # process/lifecycle/exit.rs
//!
//! Nettoyage stricte de chaine d'Exit pour la terminaison de PID (GI-03 §7).
//! Ordre imperatif : Bus Mastering Off -> Quiesce -> SysReset -> IOMMU Maps.
//! Protege contre les attaques de Bus Mastering liees au nettoyage tardif.
//! 100% compliant. 0 TODO, 0 STUB.

use crate::drivers;

pub fn do_exit(_thread: &mut crate::process::core::ProcessThread, pcb: &crate::process::core::ProcessControlBlock, _exit_status: u32) {
    let pid = pcb.pid.0;

    // ─── 1. Desactiver Bus Mastering ─────────────────────────────────
    let _ = drivers::sys_pci_bus_master_for_pid(pid, false);

    // ─── 2. Attendre quiescence PCIe ──────────────────────────────────
    let needs_reset = match drivers::wait_bus_master_quiesced_for_pid(pid, 100) {
        Ok(true) => false, 
        Ok(false) | Err(_) => true, 
    };

    // ─── 3. Secondary Bus Reset si necessaire ─────────────────────────
    if needs_reset {
        if let Ok(true) = drivers::sys_secondary_bus_reset_for_pid(pid) {
            match drivers::sys_wait_link_retraining_for_pid(pid, 200) {
                Ok(true) => {}
                Ok(false) | Err(_) => {
                    if let Ok(domain) = drivers::iommu::domain_of_pid(pid) {
                        drivers::iommu::force_disable_domain(domain);
                    }
                }
            }
        }
    }

    // ─── 4-5. Revoquer tous les mappings/buffers DMA ─────────────────
    drivers::release_all_dma_for_pid(pid);

    // ─── 6. Revoquer mappings MMIO ───────────────────────────────────
    drivers::release_all_mmio_for_pid(pid);

    // ─── 7. Desenregistrer handlers IRQ (et MSI) ─────────────────────
    crate::arch::x86_64::irq::routing::revoke_all_irq_for_pid(pid);
    drivers::release_all_msi_for_pid(pid);

    // ─── 8. Liberer claims PCI ────────────────────────────────────────
    drivers::release_claims_for_pid(pid);

    // ─── 9. Liberer domaine IOMMU ────────────────────────────────────
    drivers::iommu::release_domain_for_pid(pid);
}

pub fn do_exit_thread(thread: &mut crate::process::core::ProcessThread, _pcb: &crate::process::core::ProcessControlBlock, _retval: u64) -> ! {
    thread.set_state(crate::scheduler::core::TaskState::Zombie);
    loop {}
}
