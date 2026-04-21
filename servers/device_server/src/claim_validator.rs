use exo_syscall_abi as syscall;

use crate::registry::PciRegistry;

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

    let snapshot = registry
        .snapshot_by_bdf(bdf_raw)
        .ok_or(syscall::ENOENT)?;

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
