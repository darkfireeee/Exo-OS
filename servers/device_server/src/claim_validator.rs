// servers/device_server/src/claim_validator.rs
//
// FIX-APP-10 (Security_Application_Audit §GAP-10) : ajout d'une vérification
// de privilege avant le claim d'un périphérique PCI.
//
// Sans cette garde, tout processus Ring1 pouvant envoyer un message au
// device_server pouvait réclamer n'importe quel périphérique PCI libre.

use exo_syscall_abi as syscall;
use crate::registry::PciRegistry;

/// PIDs autorisés à réclamer des périphériques PCI.
///
/// FIX-APP-10 : équivalent de CAP_DEVICE_CLAIM (à migrer vers CapToken en v0.3).
/// En v0.2.0, seuls les serveurs de drivers statiques ont ce droit.
const DEVICE_CLAIM_ALLOWED_PIDS: &[u32] = &[
    1,   // init_server — démarre les drivers au boot
    6,   // device_server lui-même (auto-claim lors de l'enum PCI)
    8,   // virtio_drivers (block, net, sound)
    9,   // scheduler_server (clock HPET)
];

/// Classe PCI autorisée pour tous (0x00 = unclassified = pas de restriction)
#[allow(dead_code)]
const PCI_CLASS_UNRESTRICTED: u8 = 0xFF;

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

    // FIX-APP-10 : vérification de privilege avant tout accès au registre.
    // Un PID non autorisé reçoit EPERM, pas ENOENT (pas de divulgation d'info).
    if !DEVICE_CLAIM_ALLOWED_PIDS.contains(&owner_pid) {
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
