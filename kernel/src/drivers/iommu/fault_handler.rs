//! # drivers/iommu/fault_handler.rs
//!
//! Processing of faults reported by `IommuFaultQueue`.
//! Source: GI-03_Drivers_IRQ_DMA.md §4
//!
//! Logic is executed within worker/softirq context, NEVER in hard ISR,
//! as it involves heavyweight actions like IPC notification to drivers
//! or terminating corrupted PIDs.

use super::fault_queue::IOMMU_FAULT_QUEUE;
use super::pid_of_domain;
use crate::drivers::device_server_ipc;

/// Scans the IOMMU fault queue collected in ISR and applies policies.
/// Must be called regularly (e.g. via the timer tick).
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
            pci_dev,
            dom,
            ftype,
            fa
        );

        // GI-03 §4 implementation: Bind domain_id to owner PID
        // and send SIGKILL (DMA corruption).
        // Here we perform a 1:1 mapping between the IOMMU domain and process PID.
        if let Some(pid) = pid_of_domain(crate::memory::dma::core::types::IommuDomainId(dom)) {
            device_server_ipc::notify_iommu_fault_kill(pid, fa, ftype);
            let malicious_pid = crate::process::core::pid::Pid(pid);
            if let Err(_e) = crate::process::signal::send_signal_to_pid(
                malicious_pid,
                crate::process::signal::Signal::SIGKILL,
            ) {
                log::error!(
                    "Failed to deliver SIGKILL to misbehaving PID {}",
                    malicious_pid.0
                );
            } else {
                log::error!(
                    "Delivered SIGKILL to PID {} due to illegal IOMMU/DMA access",
                    malicious_pid.0
                );
            }
        }
    }

    if count > 0 {
        log::warn!("Processed {} IOMMU hardware faults this tick.", count);
    }
}
