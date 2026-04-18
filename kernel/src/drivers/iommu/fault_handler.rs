//! # drivers/iommu/fault_handler.rs
//!
//! Traitement des erreurs remontées par la `IommuFaultQueue`.
//! Source : GI-03_Drivers_IRQ_DMA.md §4
//!
//! La logique est exécutée dans le contexte du worker/softirq, JAMAIS en ISR dur,
//! car elle implique des actions lourdes comme la notification IPC aux drivers
//! ou la terminaison de PIDs corrompus.

use super::fault_queue::IOMMU_FAULT_QUEUE;
use super::pid_of_domain;
use crate::drivers::device_server_ipc;

/// Scanne la file d'erreurs IOMMU collectées en ISR et applique les politiques.
/// Doit être appelé régulièrement (ex: via le timer tick).
pub fn process_iommu_faults() {
    let dropped = IOMMU_FAULT_QUEUE.drain_dropped();
    if dropped > 0 {
        log::error!("IOMMU fault queue overflow: {} event(s) dropped", dropped);
    }

    let mut count = 0;
    while let Some(fault) = IOMMU_FAULT_QUEUE.pop() {
        count += 1;
        
        let fa = fault.faulted_addr;
        let pci_dev = fault.device_id;
        let dom = fault.domain_id;
        let ftype = fault.fault_type;

        log::error!(
            "IOMMU FAULT DETECTED: device {:#06x} domain {} type {} @ {:#018x}",
            pci_dev, dom, ftype, fa
        );

        // Implémentation GI-03 §4: Lier domain_id au PID propriétaire
        // et envoyer un SIGKILL (corruption DMA).
        // On effectue ici un mapping 1:1 entre le domaine IOMMU et le PID du processus.
        if let Some(pid) = pid_of_domain(crate::memory::dma::core::types::IommuDomainId(dom)) {
            device_server_ipc::notify_iommu_fault_kill(pid, fa, ftype);
            let malicious_pid = crate::process::core::pid::Pid(pid);
            if let Err(_e) = crate::process::signal::send_signal_to_pid(
                malicious_pid, 
                crate::process::signal::Signal::SIGKILL
            ) {
                log::error!("Failed to deliver SIGKILL to misbehaving PID {}", malicious_pid.0);
            } else {
                log::error!("Delivered SIGKILL to PID {} due to illegal IOMMU/DMA access", malicious_pid.0);
            }
        }
    }

    if count > 0 {
        log::warn!("Processed {} IOMMU hardware faults this tick.", count);
    }
}
