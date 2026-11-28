// servers/device_server/src/claim_validator.rs
//
// FIX-APP-10 (Security_Application_Audit §GAP-10) : ajout d'une vérification
// de privilege avant le claim d'un périphérique PCI.
//
// Sans cette garde, tout processus Ring1 pouvant envoyer un message au
// device_server pouvait réclamer n'importe quel périphérique PCI libre.

use exo_syscall_abi as syscall;
use crate::registry::PciRegistry;

/// FIX-14: Kernel-attested Ring 1 driver/service classes allowed to claim PCI devices.
/// Replaces the brittle hardcoded `DEVICE_CLAIM_ALLOWED_PIDS = &[1, 6, 8, 9]` list.
const DEVICE_CLAIM_ALLOWED_CLASSES: &[u8] = &[
    syscall::SVC_INIT_SERVER,
    syscall::SVC_DEVICE_SERVER,
    syscall::SVC_VIRTIO_DRIVER,
    syscall::SVC_SCHEDULER_SERVER,
    syscall::SVC_NETWORK_SERVER,
    syscall::SVC_FB_SERVER,
    syscall::SVC_AUDIO_SERVER,
    syscall::SVC_USB_SERVER,
    syscall::SVC_PS2_DRIVER,
];

pub fn validate_claim(
    registry: &PciRegistry,
    phys_base: u64,
    size: u64,
    owner_pid: u32,
    bdf_raw: u32,
    flags: u32,
) -> Result<(), i64> {
    if size == 0 || owner_pid == 0 {
        return Err(syscall::EINVAL);
    }

    // FIX-14: Verify kernel-attested ServiceClass before disclosing registry state.
    if owner_pid != 1 && !syscall::peer_is_any_service(owner_pid, DEVICE_CLAIM_ALLOWED_CLASSES) {
        return Err(syscall::EPERM);
    }

    let snapshot = registry.snapshot_by_bdf(bdf_raw).ok_or(syscall::ENOENT)?;

    if snapshot.phys_base != phys_base || snapshot.size != size {
        return Err(syscall::EINVAL);
    }

    if snapshot.owner_pid != 0 {
        return Err(syscall::EBUSY);
    }

    if (flags & 0x1) == 0 {
        return Err(syscall::EINVAL);
    }

    Ok(())
}
