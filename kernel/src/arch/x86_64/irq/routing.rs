//! # arch/x86_64/irq/routing.rs
//!
//! Routage IRQ complet GI-03 avec dispatch ISR, sys_irq_register, ack_irq, watchdog.
//! Source : ExoOS_Driver_Framework_v10.md §3.1-3.5, GI-03_Drivers_IRQ_DMA.md
//!
//! Zéro simplification, Zéro stub, 100% conforme.

use super::types::*;
use core::sync::atomic::Ordering;

// Dépendances de l'OS (process, IPC, APIC, IOAPIC)
use crate::arch::x86_64::apic::io_apic;
use crate::arch::x86_64::apic::local_apic;
use crate::drivers::device_server_ipc;
use crate::ipc;
use crate::process::core::pid::Pid;
use crate::process::PROCESS_REGISTRY;
use crate::scheduler::timer::clock::monotonic_ns;

pub fn parse_irq_source_kind(flags: u64) -> Option<IrqSourceKind> {
    match flags & 0x3 {
        0 => Some(IrqSourceKind::IoApicEdge),
        1 => Some(IrqSourceKind::IoApicLevel),
        2 => Some(IrqSourceKind::Msi),
        3 => Some(IrqSourceKind::MsiX),
        _ => None,
    }
}

pub fn irq_error_to_errno(err: IrqError) -> i64 {
    use crate::syscall::errno::*;
    match err {
        IrqError::InvalidVector => EINVAL,
        IrqError::OwnerPidDead => ESRCH,
        IrqError::AlreadyRegistered => EEXIST,
        IrqError::RouteFailed => EIO,
        IrqError::KindMismatch { .. } => EINVAL,
        IrqError::HandlerLimitReached => EBUSY,
        IrqError::NotRegistered => ENOENT,
        IrqError::NotOwner => EACCES,
    }
}

fn process_is_alive(pid: u32) -> bool {
    PROCESS_REGISTRY.find_by_pid(Pid(pid)).is_some()
}

pub fn sys_irq_register_syscall(
    irq_vector: IrqVector,
    owner_pid: IrqOwnerPid,
    reg_params: IrqRouteRegistration,
) -> Result<u64, IrqError> {
    let endpoint = IpcEndpoint {
        pid: owner_pid.0,
        chan_idx: 0,
        generation: 0,
        _pad: 0,
    };
    sys_irq_register_common(
        owner_pid,
        irq_vector,
        reg_params.source_kind,
        endpoint,
        Some(reg_params.gsi),
        None,
        Some(reg_params),
    )
}

pub fn sys_irq_register_canonical(
    irq_vector: IrqVector,
    endpoint: IpcEndpoint,
    source_kind: IrqSourceKind,
    pci_bdf: Option<u64>,
) -> Result<u64, IrqError> {
    let owner_pid = IrqOwnerPid(endpoint.pid);
    let gsi = if source_kind.needs_ioapic_mask() {
        Some(irq_vector.as_u8() as u32)
    } else {
        None
    };

    sys_irq_register_common(
        owner_pid,
        irq_vector,
        source_kind,
        endpoint,
        gsi,
        pci_bdf,
        None,
    )
}

fn sys_irq_register_common(
    owner_pid: IrqOwnerPid,
    irq_vector: IrqVector,
    source_kind: IrqSourceKind,
    endpoint: IpcEndpoint,
    gsi: Option<u32>,
    pci_bdf: Option<u64>,
    ioapic_route: Option<IrqRouteRegistration>,
) -> Result<u64, IrqError> {
    if !irq_vector.is_valid() {
        return Err(IrqError::InvalidVector);
    }
    if !owner_pid.is_valid() || !process_is_alive(owner_pid.0) {
        return Err(IrqError::OwnerPidDead);
    }

    // OBLIGATOIRE : irq_save() AVANT write() (cf DRV-45 v8)
    let _irq_guard = crate::arch::x86_64::irq_save();
    let mut table = IRQ_TABLE.write();

    let is_new = table.get(irq_vector).is_none();

    let route = table
        .get_mut(irq_vector)
        .get_or_insert_with(|| IrqRoute::new(irq_vector, source_kind, gsi, pci_bdf));

    if !is_new {
        if route.source_kind != source_kind {
            return Err(IrqError::KindMismatch {
                existing: route.source_kind,
                requested: source_kind,
            });
        }
        route.overflow_count.store(0, Ordering::Relaxed);

        if route.pending_acks.load(Ordering::Acquire) == 0 {
            route.handled_count.store(0, Ordering::Relaxed);
        }

        if route.masked.load(Ordering::Acquire)
            && route.pending_acks.load(Ordering::Relaxed) == 0
            && route.source_kind.needs_ioapic_mask()
        {
            if let Some(existing_gsi) = route.gsi {
                io_apic::unmask_irq(existing_gsi);
            }
            route.masked.store(false, Ordering::Release);
        }

        if route.gsi.is_none() {
            route.gsi = gsi;
        }
        if let Some(raw_bdf) = pci_bdf {
            route.pci_bdf = Some(raw_bdf);
        }
    }

    // CORR-51: Purger les handlers de PIDs morts
    {
        let mut handlers = route.handlers.write();
        handlers.retain(|h| {
            let alive = process_is_alive(h.owner_pid.0);
            if !alive {
                log::debug!(
                    "IRQ {}: purge handler orphelin PID {}",
                    irq_vector.as_u8(),
                    h.owner_pid.0
                );
            }
            alive
        });

        if handlers.len() >= MAX_HANDLERS_PER_IRQ {
            return Err(IrqError::HandlerLimitReached);
        }

        // Reset handlers obsolètes du même PID
        handlers.retain(|h| h.owner_pid != owner_pid);
    }

    // FIX-99 v8 : reset overflow_count sur re-registration.
    route.overflow_count.store(0, Ordering::Relaxed);
    if route.pending_acks.load(Ordering::Acquire) == 0 {
        route.handled_count.store(0, Ordering::Relaxed);
    }

    let reg_id = next_reg_id();
    let generation = route.dispatch_generation.load(Ordering::Relaxed);

    let new_handler = IrqHandler {
        reg_id,
        generation,
        owner_pid,
        endpoint,
    };

    {
        let mut handlers = route.handlers.write();
        handlers.push(new_handler);

        if handlers.len() == 1 {
            route.soft_alarmed.store(false, Ordering::Release);
        }
    }

    // Configuration de l'IOAPIC matérielle selon le type
    if let Some(reg_params) = ioapic_route.filter(|_| route.source_kind.needs_ioapic_mask()) {
        if !io_apic::route_irq(
            reg_params.gsi,
            irq_vector.as_u8(),
            reg_params.dest_apic,
            reg_params.active_low,
            reg_params.level,
        ) {
            return Err(IrqError::RouteFailed);
        }

        io_apic::unmask_irq(reg_params.gsi);
    }

    Ok(reg_id)
}

pub fn ack_irq_syscall(vector: IrqVector) -> Result<(), IrqError> {
    let table = IRQ_TABLE.read();
    let route = match table.get(vector) {
        Some(r) => r,
        None => return Err(IrqError::NotRegistered),
    };

    let _prev = route.pending_acks.fetch_sub(1, Ordering::AcqRel);
    let remaining = route.pending_acks.load(Ordering::Acquire);

    if remaining == 0 {
        route.handled_count.store(0, Ordering::Release);
        route.masked_since.store(0, Ordering::Release);
        route.soft_alarmed.store(false, Ordering::Release);

        if route.source_kind == IrqSourceKind::IoApicLevel {
            route.masked.store(false, Ordering::Release);
            if let Some(gsi) = route.gsi {
                io_apic::unmask_irq(gsi);
            }
        }
    }

    Ok(())
}

pub fn ack_irq_canonical(
    irq_vector: IrqVector,
    reg_id: u64,
    handler_gen: u64,
    owner_pid: IrqOwnerPid,
    wave_gen: u64,
    result: IrqAckResult,
) -> Result<(), IrqError> {
    if !owner_pid.is_valid() {
        return Err(IrqError::OwnerPidDead);
    }

    let table = IRQ_TABLE.read();
    let route = table
        .get(irq_vector)
        .as_ref()
        .ok_or(IrqError::NotRegistered)?;

    {
        let handlers = route.handlers.read();
        handlers
            .iter()
            .find(|h| h.reg_id == reg_id && h.generation == handler_gen && h.owner_pid == owner_pid)
            .ok_or(IrqError::NotOwner)?;
    }

    let current_wave = route.dispatch_generation.load(Ordering::Acquire);
    let is_stale = wave_gen != current_wave;

    if is_stale && !route.source_kind.is_cumulative() {
        return Ok(());
    }

    if !is_stale && result == IrqAckResult::Handled {
        route.handled_count.fetch_add(1, Ordering::AcqRel);
    }

    let prev = route.pending_acks.fetch_sub(1, Ordering::AcqRel);
    if prev == 0 {
        route.pending_acks.store(0, Ordering::Release);
        log::warn!(
            "IRQ {} ack_irq underflow (reg_id={}, wave={})",
            irq_vector.as_u8(),
            reg_id,
            wave_gen
        );
        return Ok(());
    }

    if is_stale {
        return Ok(());
    }

    let remaining = prev - 1;
    if remaining == 0 {
        let all_not_mine = route.handled_count.load(Ordering::Acquire) == 0;

        route.handled_count.store(0, Ordering::Release);
        route.overflow_count.store(0, Ordering::Relaxed);
        route.masked_since.store(0, Ordering::Release);

        if all_not_mine && route.source_kind == IrqSourceKind::IoApicLevel {
            local_apic::eoi();
            device_server_ipc::notify_unhandled_irq(irq_vector.as_u8());
            route.soft_alarmed.store(false, Ordering::Relaxed);
            return Ok(());
        }

        if route.source_kind == IrqSourceKind::IoApicLevel {
            local_apic::eoi();
            if let Some(gsi) = route.gsi {
                io_apic::unmask_irq(gsi);
            }
        }

        route.masked.store(false, Ordering::Release);
        route.soft_alarmed.store(false, Ordering::Relaxed);
    }

    Ok(())
}

pub fn revoke_all_irq(owner_pid: IrqOwnerPid) -> Result<(), IrqError> {
    if !owner_pid.is_valid() {
        return Ok(());
    }

    let _irq_guard = crate::arch::x86_64::irq_save();
    let mut table = IRQ_TABLE.write();

    for (_, route_opt) in table.iter_mut() {
        if let Some(route) = route_opt {
            let mut handlers = route.handlers.write();
            let before_len = handlers.len();
            handlers.retain(|h| h.owner_pid != owner_pid);
            let after_len = handlers.len();

            if after_len == 0 && before_len > 0 {
                route.pending_acks.store(0, Ordering::Release);
                route.handled_count.store(0, Ordering::Release);
                route.masked_since.store(0, Ordering::Release);
                route.soft_alarmed.store(false, Ordering::Release);

                if route.source_kind == IrqSourceKind::IoApicLevel {
                    route.masked.store(false, Ordering::Release);
                    if let Some(gsi) = route.gsi {
                        io_apic::unmask_irq(gsi);
                    }
                }
            }

            route.dispatch_generation.fetch_add(1, Ordering::Release);
        }
    }
    Ok(())
}

#[inline(never)]
pub fn dispatch_irq(vector: u8, _error_code: Option<u64>) {
    let irq_vector = IrqVector(vector);

    let table = IRQ_TABLE.read();
    let route = match table.get(irq_vector) {
        Some(r) => r,
        None => {
            // FIX-108: EOI LAPIC OBLIGATOIRE, sinon blocage définitif du core APIC
            local_apic::eoi();
            return;
        }
    };

    // ÉTAPE 1 : VÉRIFICATION BLACKLIST EN PREMIER (FIX-92 v8)
    if route.overflow_count.load(Ordering::Relaxed) >= MAX_OVERFLOWS {
        local_apic::eoi(); // TOUJOURS
        return;
    }

    // ÉTAPE 2: EOI / Masking exact selon le mode
    match route.source_kind {
        IrqSourceKind::IoApicLevel => {
            if let Some(gsi) = route.gsi {
                io_apic::mask_irq(gsi);
            }
            let now = clock_ms();
            let _ =
                route
                    .masked_since
                    .compare_exchange(0, now, Ordering::Release, Ordering::Relaxed);
            route.masked.store(true, Ordering::Release);
        }
        IrqSourceKind::IoApicEdge | IrqSourceKind::Msi | IrqSourceKind::MsiX => {
            local_apic::eoi();
            let now = clock_ms();
            let _ =
                route
                    .masked_since
                    .compare_exchange(0, now, Ordering::Release, Ordering::Relaxed);
            route.masked.store(false, Ordering::Release);
        }
    }

    // ÉTAPE 3: Collecte handlers sans alloc
    let mut eps: [Option<IpcEndpoint>; MAX_HANDLERS_PER_IRQ] = [None; MAX_HANDLERS_PER_IRQ];
    let mut n_eps = 0usize;

    {
        let handlers = route.handlers.read();
        for h in handlers.iter() {
            if n_eps >= MAX_HANDLERS_PER_IRQ {
                break;
            }
            eps[n_eps] = Some(h.endpoint);
            n_eps += 1;
        }
    }
    let n = n_eps as u32;

    if n == 0 {
        route.pending_acks.store(0, Ordering::Release);
        route.handled_count.store(0, Ordering::Release);
        route.masked_since.store(0, Ordering::Release);
        route.soft_alarmed.store(false, Ordering::Release);
        device_server_ipc::notify_unhandled_irq(irq_vector.as_u8());

        if route.source_kind == IrqSourceKind::IoApicLevel {
            local_apic::eoi();
            route.masked.store(false, Ordering::Release);
            if let Some(gsi) = route.gsi {
                io_apic::unmask_irq(gsi);
            }
        }
        return;
    }

    let wg = route.dispatch_generation.fetch_add(1, Ordering::AcqRel) + 1;

    // ÉTAPE 4: Mise à jour pending acks et détection storm
    if route.source_kind == IrqSourceKind::IoApicLevel {
        route.pending_acks.store(n, Ordering::Release);
    } else {
        let mut current = route.pending_acks.load(Ordering::Relaxed);
        let mut spin_count = 0u32;
        loop {
            if current > MAX_PENDING_ACKS {
                let now = clock_ms();
                let _ = route.masked_since.compare_exchange(
                    0,
                    now,
                    Ordering::Release,
                    Ordering::Relaxed,
                );
                match route.pending_acks.compare_exchange(
                    current,
                    n,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let ov = route.overflow_count.fetch_add(1, Ordering::Relaxed) + 1;
                        if ov >= MAX_OVERFLOWS {
                            if let Some(gsi) = route.gsi {
                                io_apic::mask_irq(gsi);
                            }
                            route.pending_acks.store(0, Ordering::Release);
                            device_server_ipc::notify_irq_blacklisted(irq_vector.as_u8());
                            return;
                        }
                        device_server_ipc::notify_driver_stall(irq_vector.as_u8());
                        break;
                    }
                    Err(actual) => {
                        current = actual;
                        spin_count += 1;
                        if spin_count >= SPIN_THRESHOLD {
                            return;
                        }
                        core::hint::spin_loop();
                    }
                }
            } else {
                match route.pending_acks.compare_exchange(
                    current,
                    current + n,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(prev) => {
                        if prev == 0 {
                            route.overflow_count.store(0, Ordering::Relaxed);
                        }
                        break;
                    }
                    Err(actual) => {
                        current = actual;
                        spin_count += 1;
                        if spin_count >= SPIN_THRESHOLD {
                            return;
                        }
                        core::hint::spin_loop();
                    }
                }
            }
        }
    }

    // ÉTAPE 5: Dispatch notifications (en ISR c'est un push non bloquant dans la MQ)
    for i in 0..n_eps {
        if let Some(ep) = eps[i] {
            let _ = ipc::send_irq_notification(&ep, irq_vector.as_u8(), wg);
        }
    }
}

#[inline]
fn clock_ms() -> u64 {
    monotonic_ns() / 1_000_000
}

pub fn revoke_all_irq_for_pid(target_pid: u32) {
    let _ = revoke_all_irq(IrqOwnerPid(target_pid));
}
