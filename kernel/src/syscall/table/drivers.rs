//! # syscall/table/drivers.rs — Handlers des pilotes matériels GI-03 (DMA, PCI, IRQ, MSI, MMIO)

use super::common::*;
use crate::arch::x86_64::irq::{
    irq_error_to_errno, parse_irq_source_kind, IpcEndpoint, IrqAckResult, IrqOwnerPid, IrqVector,
};
use crate::drivers::{ClaimError, MmioError, MsiError, PciCfgError, TopoError};
use crate::memory::core::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, IommuDomainId, IovaAddr,
};
use crate::process::core::pid::Pid;
use crate::syscall::numbers::*;
use crate::syscall::validation::write_user_typed;
use pci_types::PciAddress;

// ─────────────────────────────────────────────────────────────────────────────
// Handlers GI-03 Drivers (530–549)
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Helpers pour DMA/PCI (réutilisables)
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn parse_dma_direction(raw: u64) -> Option<DmaDirection> {
    match raw {
        0 => Some(DmaDirection::ToDevice),
        1 => Some(DmaDirection::FromDevice),
        2 => Some(DmaDirection::Bidirection),
        3 => Some(DmaDirection::None),
        _ => None,
    }
}

#[inline]
fn parse_irq_ack_result(raw: u64) -> Option<IrqAckResult> {
    match raw {
        0 => Some(IrqAckResult::Handled),
        1 => Some(IrqAckResult::NotMine),
        _ => None,
    }
}

#[inline]
fn parse_ipc_endpoint_packed(lo: u64, hi: u64) -> IpcEndpoint {
    IpcEndpoint {
        pid: lo as u32,
        chan_idx: (lo >> 32) as u32,
        generation: hi as u32,
        _pad: (hi >> 32) as u32,
    }
}

#[inline]
fn normalize_driver_irq(
    raw_irq: u64,
    source_kind: crate::arch::x86_64::irq::IrqSourceKind,
) -> Result<IrqVector, i64> {
    if raw_irq > u8::MAX as u64 {
        return Err(EINVAL);
    }
    let raw = raw_irq as u8;
    let vector = if source_kind.needs_ioapic_mask() && raw < IrqVector::VECTOR_IRQ_BASE {
        raw.saturating_add(IrqVector::VECTOR_IRQ_BASE)
    } else {
        raw
    };
    let vector = IrqVector(vector);
    if vector.is_valid() {
        Ok(vector)
    } else {
        Err(EINVAL)
    }
}

#[inline]
fn caller_is_ps2_driver(caller_pid: u32) -> bool {
    matches!(
        crate::security::service_class_of(Pid(caller_pid)),
        crate::security::ServiceClass::Ps2Driver
    )
}

#[inline]
fn ps2_ioport_allowed(port: u64, width: u64) -> bool {
    width == 1 && matches!(port, 0x60 | 0x64)
}

#[inline]
fn ps2_keyboard_vector() -> u8 {
    IrqVector::VECTOR_IRQ_BASE + 1
}

/// Encode/decode ABI simplifié BDF sur 32 bits :
/// - bits 31..16 : segment
/// - bits 15..8  : bus
/// - bits 7..3   : device
/// - bits 2..0   : function
#[inline]
fn parse_pci_address(raw: u64) -> PciAddress {
    let v = raw as u32;
    let segment = (v >> 16) as u16;
    let bus = ((v >> 8) & 0xFF) as u8;
    let device = ((v >> 3) & 0x1F) as u8;
    let function = (v & 0x07) as u8;
    PciAddress::new(segment, bus, device, function)
}

#[inline]
fn dma_error_to_errno(err: DmaError) -> i64 {
    match err {
        DmaError::OutOfMemory => ENOMEM,
        DmaError::NoChannel
        | DmaError::Timeout
        | DmaError::NotInitialized
        | DmaError::AlreadySubmitted
        | DmaError::Cancelled => EAGAIN,
        DmaError::HardwareError | DmaError::IommuFault => EFAULT,
        DmaError::InvalidParams
        | DmaError::MisalignedBuffer
        | DmaError::WrongZone
        | DmaError::NotSupported => EINVAL,
    }
}

#[inline]
fn claim_error_to_errno(err: ClaimError) -> i64 {
    match err {
        ClaimError::PermissionDenied => EACCES,
        ClaimError::AlreadyClaimed => EBUSY,
        ClaimError::NotInHardwareRegion | ClaimError::PhysIsRam => EINVAL,
        ClaimError::TableFull => ENOMEM,
    }
}

#[inline]
fn topo_error_to_errno(err: TopoError) -> i64 {
    match err {
        TopoError::TopologyTableFull => ENOMEM,
    }
}

#[inline]
fn pci_cfg_error_to_errno(err: PciCfgError) -> i64 {
    match err {
        PciCfgError::NotClaimed => EPERM,
        PciCfgError::PermissionDenied => EACCES,
    }
}

#[inline]
fn mmio_error_to_errno(err: MmioError) -> i64 {
    match err {
        MmioError::PermissionDenied => EACCES,
        MmioError::AlreadyMapped => EBUSY,
        MmioError::OutOfMemory => ENOMEM,
        MmioError::NotMapped => EFAULT,
        MmioError::InvalidParams => EINVAL,
    }
}

#[inline]
fn msi_error_to_errno(err: MsiError) -> i64 {
    match err {
        MsiError::NotFound => ENOENT,
        MsiError::TableFull | MsiError::NoSpace => ENOMEM,
        MsiError::AmbiguousClaim => EINVAL,
        MsiError::InvalidParams => EINVAL,
    }
}

/// ABI GI-03 canonique.
/// Encodage registre noyau :
/// `sys_irq_register(irq, endpoint_lo, endpoint_hi, source_kind, bdf_raw, has_bdf)`.
/// `endpoint_lo = pid | (chan_idx << 32)`, `endpoint_hi = generation | (_pad << 32)`.
pub fn sys_irq_register(
    irq: u64,
    endpoint_lo: u64,
    endpoint_hi: u64,
    source_kind: u64,
    bdf_raw: u64,
    has_bdf: u64,
) -> i64 {
    stat_inc(SYS_IRQ_REGISTER);

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let source_kind = match parse_irq_source_kind(source_kind) {
        Some(kind) => kind,
        None => return EINVAL,
    };
    let vector = match normalize_driver_irq(irq, source_kind) {
        Ok(vector) => vector,
        Err(errno) => return errno,
    };

    let mut endpoint = parse_ipc_endpoint_packed(endpoint_lo, endpoint_hi);
    endpoint.pid = caller_pid;

    let bdf = if has_bdf != 0 { Some(bdf_raw) } else { None };

    match crate::arch::x86_64::irq::sys_irq_register(vector, endpoint, source_kind, bdf) {
        Ok(reg_id) => {
            if vector.as_u8() == ps2_keyboard_vector() && caller_is_ps2_driver(caller_pid) {
                crate::arch::x86_64::terminal::handoff_keyboard_to_ring1();
            }
            reg_id as i64
        }
        Err(err) => irq_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_irq_ack(irq, reg_id, handler_gen, wave_gen, result)`.
pub fn sys_irq_ack(
    irq: u64,
    reg_id: u64,
    handler_gen: u64,
    wave_gen: u64,
    result_raw: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_IRQ_ACK);
    if irq > u8::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let vector = IrqVector(irq as u8);
    if !vector.is_valid() {
        return EINVAL;
    }

    let result = match parse_irq_ack_result(result_raw) {
        Some(result) => result,
        None => return EINVAL,
    };

    match crate::arch::x86_64::irq::ack_irq(
        vector,
        reg_id,
        handler_gen,
        IrqOwnerPid(caller_pid),
        wave_gen,
        result,
    ) {
        Ok(()) => 0,
        Err(err) => irq_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_ioport_read(port, width)`.
///
/// Transition terminal v0.2.0: seul le driver Ring1 `ps2_driver` peut accéder
/// aux ports i8042 clavier (0x60/0x64), en octets.
pub fn sys_ioport_read(port: u64, width: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_IOPORT_READ);

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || !caller_is_ps2_driver(caller_pid) {
        return EACCES;
    }
    if !ps2_ioport_allowed(port, width) {
        return EINVAL;
    }

    unsafe { crate::arch::x86_64::inb(port as u16) as i64 }
}

/// ABI GI-03 : `sys_ioport_write(port, value, width)`.
pub fn sys_ioport_write(port: u64, value: u64, width: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_IOPORT_WRITE);

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || !caller_is_ps2_driver(caller_pid) {
        return EACCES;
    }
    if !ps2_ioport_allowed(port, width) || value > u8::MAX as u64 {
        return EINVAL;
    }

    unsafe {
        crate::arch::x86_64::outb(port as u16, value as u8);
    }
    0
}

/// ABI GI-03 : `sys_mmio_map(phys_addr, size)` pour le PID appelant.
pub fn sys_mmio_map(phys_addr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MMIO_MAP);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_mmio_map_for_pid(caller_pid, PhysAddr::new(phys_addr), size as usize)
    {
        Ok(virt) => virt as i64,
        Err(err) => mmio_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_mmio_unmap(virt_addr, size)` pour le PID appelant.
pub fn sys_mmio_unmap(virt_addr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MMIO_UNMAP);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_mmio_unmap_for_pid(caller_pid, virt_addr, size as usize) {
        Ok(()) => 0,
        Err(err) => mmio_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_dma_alloc(size, direction) -> (virt, iova)`.
/// Réalisation registre noyau :
/// - `rax` retourne l'IOVA
/// - `arg3` (`user_virt_out`) reçoit l'adresse CPU virtuelle si non nul
/// - `arg4`/`arg5` restent des extensions de compatibilité (`map_flags`, `domain_hint`)
pub fn sys_dma_alloc(
    size: u64,
    direction: u64,
    user_virt_out: u64,
    map_flags: u64,
    domain_hint: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_DMA_ALLOC);

    if size == 0 || size > usize::MAX as u64 || domain_hint > u32::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let requested_domain = IommuDomainId(domain_hint as u32);
    let effective_domain = match crate::drivers::iommu::ensure_domain_for_pid(caller_pid) {
        Ok(domain) => domain,
        Err(_) if requested_domain.0 != 0 => requested_domain,
        Err(_) => return EAGAIN,
    };

    match crate::drivers::sys_dma_alloc_for_pid(
        caller_pid,
        size as usize,
        direction,
        DmaMapFlags(map_flags as u32),
        effective_domain,
    ) {
        Ok((virt, iova)) => {
            if user_virt_out != 0 {
                if let Err(e) = write_user_typed::<u64>(user_virt_out, virt) {
                    let _ =
                        crate::drivers::sys_dma_free_for_pid(caller_pid, iova, effective_domain);
                    return e.to_errno();
                }
            }
            iova.0 as i64
        }
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_dma_free(iova, size)`.
/// `arg3` reste un hint de domaine pour le noyau tant que la couche userland n'est pas finalisée.
pub fn sys_dma_free(iova: u64, size: u64, domain_hint: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_FREE);

    if size == 0 || size > usize::MAX as u64 || domain_hint > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    if crate::drivers::dma::dma_alloc_size_for_pid(caller_pid, IovaAddr(iova))
        != Some(size as usize)
    {
        return EINVAL;
    }

    let requested_domain = IommuDomainId(domain_hint as u32);
    let effective_domain =
        crate::drivers::iommu::domain_of_pid(caller_pid).unwrap_or(requested_domain);

    match crate::drivers::sys_dma_free_for_pid(caller_pid, IovaAddr(iova), effective_domain) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 canonique : `sys_dma_sync(iova, size, direction)`.
pub fn sys_dma_sync(iova: u64, size: u64, direction: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_SYNC);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_dma_sync_for_pid(caller_pid, IovaAddr(iova), size as usize, direction)
    {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_cfg_read(offset)` pour le device claimé du PID appelant.
pub fn sys_pci_cfg_read(offset: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_CFG_READ);

    if offset > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_cfg_read_for_pid(caller_pid, offset as u16) {
        Ok(value) => value as i64,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_cfg_write(offset, value)` pour le device claimé du PID appelant.
pub fn sys_pci_cfg_write(offset: u64, value: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_CFG_WRITE);

    if offset > u16::MAX as u64 || value > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_cfg_write_for_pid(caller_pid, offset as u16, value as u32) {
        Ok(()) => 0,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_bus_master(enable)` pour le device claimé du PID appelant.
pub fn sys_pci_bus_master(enable: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_BUS_MASTER);

    if enable > 1 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_bus_master_for_pid(caller_pid, enable != 0) {
        Ok(()) => 0,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_claim(phys_addr, size, owner_pid, bdf_raw, has_bdf)`.
pub fn sys_pci_claim(
    phys_addr: u64,
    size: u64,
    owner_pid: u64,
    bdf_raw: u64,
    has_bdf: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PCI_CLAIM);
    if size == 0 || size > usize::MAX as u64 || owner_pid == 0 || owner_pid > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let owner_pid = owner_pid as u32;
    let bdf = if has_bdf != 0 {
        Some(parse_pci_address(bdf_raw))
    } else {
        None
    };

    if bdf.is_none()
        && owner_pid == caller_pid
        && crate::security::service_class_of(Pid(caller_pid))
            == crate::security::ServiceClass::FbServer
        && crate::arch::x86_64::framebuffer_early::user_framebuffer_contains(phys_addr, size)
    {
        return match crate::drivers::device_claims::claim_trusted_mmio_for_pid(
            PhysAddr::new(phys_addr),
            size as usize,
            owner_pid,
        ) {
            Ok(()) => {
                if crate::drivers::iommu::ensure_domain_for_pid(owner_pid).is_err() {
                    let _ = crate::drivers::release_claim_for_owner(owner_pid);
                    return EAGAIN;
                }
                0
            }
            Err(err) => claim_error_to_errno(err),
        };
    }

    match crate::drivers::sys_pci_claim(
        PhysAddr::new(phys_addr),
        size as usize,
        owner_pid,
        bdf,
        caller_pid,
    ) {
        Ok(()) => {
            if crate::drivers::iommu::ensure_domain_for_pid(owner_pid).is_err() {
                let _ = crate::drivers::release_claim_for_owner(owner_pid);
                return EAGAIN;
            }
            0
        }
        Err(err) => claim_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_dma_map(vaddr, size, direction)`.
pub fn sys_dma_map(vaddr: u64, size: u64, direction: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_MAP);

    if size == 0 || size > usize::MAX as u64 || vaddr > usize::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_dma_map(caller_pid, vaddr as usize, size as usize, direction) {
        Ok(iova) => iova.0 as i64,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_dma_unmap(domain_id, iova)`.
pub fn sys_dma_unmap(domain_id: u64, iova: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_UNMAP);
    if domain_id > u32::MAX as u64 {
        return EINVAL;
    }

    let requested_domain = IommuDomainId(domain_id as u32);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let effective_domain = if caller_pid != 0 {
        crate::drivers::iommu::domain_of_pid(caller_pid).unwrap_or(requested_domain)
    } else {
        requested_domain
    };

    match crate::drivers::sys_dma_unmap(caller_pid, IovaAddr(iova), effective_domain) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_alloc(count)`.
pub fn sys_msi_alloc(count: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_ALLOC);

    if count == 0 || count > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_alloc_for_pid(caller_pid, count as u16) {
        Ok(handle) => handle as i64,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_config(handle, vector_idx)`.
pub fn sys_msi_config(handle: u64, vector_idx: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_CONFIG);

    if vector_idx > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_config_for_pid(caller_pid, handle, vector_idx as u16) {
        Ok(_vector) => 0,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_free(handle)`.
pub fn sys_msi_free(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_FREE);

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_free_for_pid(caller_pid, handle) {
        Ok(()) => 0,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 canonique : `sys_pci_set_topology(child_bdf_raw, parent_bdf_raw)`.
pub fn sys_pci_set_topology(
    bdf_raw: u64,
    parent_bdf_raw: u64,
    has_parent: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PCI_SET_TOPOLOGY);

    if has_parent == 0 {
        return EINVAL;
    }

    let address = parse_pci_address(bdf_raw);
    let parent_bridge = parse_pci_address(parent_bdf_raw);

    match crate::drivers::sys_pci_set_topology(address, parent_bridge) {
        Ok(()) => 0,
        Err(err) => topo_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_find_device(vendor, device, class, subclass, index, out_ptr)`.
///
/// `vendor`/`device` à 0 signifient wildcard. `class`/`subclass` à une valeur
/// supérieure à 255 signifient wildcard. Le résultat est écrit dans
/// `PciDeviceInfo` côté userspace et contient le BDF, IRQ et BAR0.
pub fn sys_pci_find_device(
    vendor_filter: u64,
    device_filter: u64,
    class_filter: u64,
    subclass_filter: u64,
    index: u64,
    out_ptr: u64,
) -> i64 {
    stat_inc(SYS_PCI_FIND_DEVICE);

    if vendor_filter > u16::MAX as u64
        || device_filter > u16::MAX as u64
        || class_filter > u16::MAX as u64
        || subclass_filter > u16::MAX as u64
        || index > u32::MAX as u64
        || out_ptr == 0
    {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let Some(info) = crate::drivers::find_pci_device(
        vendor_filter as u16,
        device_filter as u16,
        class_filter as u16,
        subclass_filter as u16,
        index as u32,
    ) else {
        return ENOENT;
    };

    match write_user_typed::<crate::drivers::PciDeviceInfo>(out_ptr, info) {
        Ok(()) => 0,
        Err(err) => err.to_errno(),
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FramebufferInfoWire {
    pub phys_addr: u64,
    pub width: u32,
    pub height: u32,
    pub stride_pixels: u32,
    pub bpp: u32,
    pub format: u32,
    pub _pad: u32,
    pub size_bytes: u64,
}

/// `framebuffer_info(out)` -> infos du framebuffer de boot pour fb_server Ring1.
/// SYS_FRAMEBUFFER_INFO — exposé uniquement aux processus Ring1 de confiance.
///
/// SECURITY: Ce syscall retourne l'adresse physique du framebuffer.
/// Seul le premier appelant (fb_server) est autorise ; tout autre PID
/// recoit EACCES. L'autorisation est stockee dans FB_INFO_AUTHORIZED_PID.
static FB_INFO_AUTHORIZED_PID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

pub fn sys_framebuffer_info(out_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use core::sync::atomic::Ordering;
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    // Autoriser uniquement le premier appelant (fb_server au boot).
    // Tout PID different est rejete avec EACCES.
    let prev =
        FB_INFO_AUTHORIZED_PID.compare_exchange(0, caller_pid, Ordering::AcqRel, Ordering::Acquire);
    match prev {
        Ok(_) => {}                                       // premier appelant autorise
        Err(authorized) if authorized == caller_pid => {} // rappel idempotent
        Err(_) => return EACCES,                          // PID non autorise
    }
    stat_inc(SYS_FRAMEBUFFER_INFO);
    if out_ptr == 0 {
        return EFAULT;
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }
    let Some((phys_addr, width, height, stride_pixels, bpp, format, size_bytes)) =
        crate::arch::x86_64::framebuffer_early::user_framebuffer_info()
    else {
        return ENOENT;
    };
    if size_bytes == 0 || size_bytes > usize::MAX as u64 {
        return EINVAL;
    }

    if let Err(err) = crate::drivers::device_claims::claim_trusted_mmio_for_pid(
        PhysAddr::new(phys_addr),
        size_bytes as usize,
        caller_pid,
    ) {
        return claim_error_to_errno(err);
    }

    let info = FramebufferInfoWire {
        phys_addr,
        width,
        height,
        stride_pixels,
        bpp,
        format,
        _pad: 0,
        size_bytes,
    };
    match write_user_typed(out_ptr, info) {
        Ok(()) => 0,
        Err(_) => EFAULT,
    }
}

