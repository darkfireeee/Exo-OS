//! # syscall/table/ipc/endpoints.rs — IPC endpoints and mailboxes management and registration

use crate::syscall::table::common::*;
use crate::ipc::core::types::EndpointId;
use crate::process::core::pid::Pid;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, UserBuf};

const IPC_ENDPOINT_OWNER_SLOTS: usize = 128;

#[derive(Clone, Copy)]
pub(super) struct IpcEndpointOwner {
    pub(super) endpoint: u64,
    pub(super) owner_pid: u32,
}

impl IpcEndpointOwner {
    pub(super) const EMPTY: Self = Self {
        endpoint: 0,
        owner_pid: 0,
    };
}

pub(super) static IPC_ENDPOINT_OWNERS: spin::Mutex<[IpcEndpointOwner; IPC_ENDPOINT_OWNER_SLOTS]> =
    spin::Mutex::new([IpcEndpointOwner::EMPTY; IPC_ENDPOINT_OWNER_SLOTS]);

#[inline]
pub(super) fn service_class_for_endpoint_name(name: &[u8]) -> Option<crate::security::ServiceClass> {
    match name {
        b"memory_server" => Some(crate::security::ServiceClass::MemoryServer),
        b"vfs_server" => Some(crate::security::ServiceClass::VfsServer),
        b"crypto_server" => Some(crate::security::ServiceClass::CryptoServer),
        b"device_server" => Some(crate::security::ServiceClass::DeviceServer),
        b"virtio_drivers" | b"e1000_driver" | b"virtio_net_driver" | b"loopback_driver" => {
            Some(crate::security::ServiceClass::VirtioDriver)
        }
        b"ps2_driver" => Some(crate::security::ServiceClass::Ps2Driver),
        b"network_server" => Some(crate::security::ServiceClass::NetworkServer),
        b"scheduler_server" => Some(crate::security::ServiceClass::SchedulerServer),
        b"input_server" => Some(crate::security::ServiceClass::InputServer),
        b"tty_server" | b"tty_input_stream" => Some(crate::security::ServiceClass::TtyServer),
        b"fb_server" | b"fb_server_input" => Some(crate::security::ServiceClass::FbServer),
        b"ipc_router" => Some(crate::security::ServiceClass::IpcBroker),
        b"exo_shield" => Some(crate::security::ServiceClass::ExoShield),
        b"exosh" | b"exosh_input_reply" => Some(crate::security::ServiceClass::Exosh),
        b"exo_compositor" | b"exo_compositor_input" => {
            Some(crate::security::ServiceClass::ExoCompositor)
        }
        b"audio_server" => Some(crate::security::ServiceClass::AudioServer),
        b"usb_server" => Some(crate::security::ServiceClass::UsbServer),
        _ => None,
    }
}

#[cfg(test)]
mod ipc_service_endpoint_class_tests {
    use super::*;
    use crate::security::ServiceClass;

    #[test]
    fn network_driver_service_endpoints_register_driver_class() {
        for name in [
            b"virtio_drivers".as_slice(),
            b"e1000_driver".as_slice(),
            b"virtio_net_driver".as_slice(),
            b"loopback_driver".as_slice(),
        ] {
            assert_eq!(
                service_class_for_endpoint_name(name),
                Some(ServiceClass::VirtioDriver)
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReserveEndpointOwnerOutcome {
    ReservedNew { evicted_dead_endpoint: Option<u64> },
    ReplacedDead,
    AlreadyOwned,
}

/// FIX-03: Validates that a process's normalized PCB binary name matches the
/// canonical Ring 1 / system binary allowed to claim `class`.
#[cfg_attr(test, allow(dead_code))]
fn pcb_name_matches_service_class(
    name_snap: &[u8; crate::process::core::pcb::PROCESS_NAME_LEN],
    class: crate::security::ServiceClass,
) -> bool {
    let mut len = 0usize;
    while len < name_snap.len() && name_snap[len] != 0 {
        len += 1;
    }
    let n = &name_snap[..len];
    use crate::security::ServiceClass;
    match class {
        ServiceClass::IpcBroker => n.starts_with(b"ipc-router") || n.starts_with(b"ipc_router"),
        ServiceClass::MemoryServer => {
            n.starts_with(b"memory_server") || n.starts_with(b"memory-server")
        }
        ServiceClass::VfsServer => n.starts_with(b"vfs_server") || n.starts_with(b"vfs-server"),
        ServiceClass::CryptoServer => {
            n.starts_with(b"crypto_server") || n.starts_with(b"crypto-server")
        }
        ServiceClass::DeviceServer => {
            n.starts_with(b"device_server") || n.starts_with(b"device-server")
        }
        ServiceClass::VirtioDriver => {
            n.starts_with(b"virtio") || n.starts_with(b"e1000") || n.starts_with(b"loopback")
        }
        ServiceClass::Ps2Driver => n.starts_with(b"ps2"),
        ServiceClass::NetworkServer => {
            n.starts_with(b"network_server") || n.starts_with(b"network-server")
        }
        ServiceClass::SchedulerServer => {
            n.starts_with(b"scheduler_serv") || n.starts_with(b"scheduler-serv")
        }
        ServiceClass::InputServer => {
            n.starts_with(b"input_server") || n.starts_with(b"input-server")
        }
        ServiceClass::TtyServer => n.starts_with(b"tty_server") || n.starts_with(b"tty-server"),
        ServiceClass::FbServer => n.starts_with(b"fb_server") || n.starts_with(b"fb-server"),
        ServiceClass::ExoShield => n.starts_with(b"exo-shield") || n.starts_with(b"exo_shield"),
        ServiceClass::Exosh => n.starts_with(b"exosh"),
        ServiceClass::ExoCompositor => {
            n.starts_with(b"exo_compositor") || n.starts_with(b"exo-compositor")
        }
        ServiceClass::AudioServer => {
            n.starts_with(b"audio_server") || n.starts_with(b"audio-server")
        }
        ServiceClass::UsbServer => n.starts_with(b"usb_server") || n.starts_with(b"usb-server"),
        ServiceClass::InitServer | ServiceClass::Unknown => false,
    }
}

pub(super) fn reserve_ipc_endpoint_owner(
    endpoint: u64,
    owner_pid: u32,
    is_system: bool,
) -> Result<ReserveEndpointOwnerOutcome, i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }
    #[cfg(not(test))]
    if !is_system && packed_pid == 0 && endpoint != owner_pid as u64 {
        return Err(EACCES);
    }
    let _ = is_system;

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut empty_slot = None;
    let mut dead_slot: Option<(usize, u64)> = None;
    let mut idx = 0usize;
    while idx < owners.len() {
        let entry = owners[idx];
        if entry.endpoint == endpoint {
            if entry.owner_pid == owner_pid {
                return Ok(ReserveEndpointOwnerOutcome::AlreadyOwned);
            }
            // FIX-03: Never allow overwriting an endpoint owned by a live process,
            // even when `is_system` is true.
            if !crate::process::is_alive(entry.owner_pid) {
                owners[idx] = IpcEndpointOwner {
                    endpoint,
                    owner_pid,
                };
                return Ok(ReserveEndpointOwnerOutcome::ReplacedDead);
            }
            return Err(EBUSY);
        }
        if entry.endpoint == 0 && empty_slot.is_none() {
            empty_slot = Some(idx);
        } else if entry.endpoint != 0
            && dead_slot.is_none()
            && !crate::process::is_alive(entry.owner_pid)
        {
            dead_slot = Some((idx, entry.endpoint));
        }
        idx += 1;
    }

    if let Some(slot) = empty_slot {
        owners[slot] = IpcEndpointOwner {
            endpoint,
            owner_pid,
        };
        return Ok(ReserveEndpointOwnerOutcome::ReservedNew {
            evicted_dead_endpoint: None,
        });
    }
    if let Some((slot, evicted_ep)) = dead_slot {
        owners[slot] = IpcEndpointOwner {
            endpoint,
            owner_pid,
        };
        return Ok(ReserveEndpointOwnerOutcome::ReservedNew {
            evicted_dead_endpoint: Some(evicted_ep),
        });
    }
    Err(ENOMEM)
}

pub(super) fn release_ipc_endpoint_owner(endpoint: u64, owner_pid: u32) -> Result<(), i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut idx = 0usize;
    while idx < owners.len() {
        if owners[idx].endpoint == endpoint {
            if owners[idx].owner_pid != owner_pid {
                return Err(EACCES);
            }
            owners[idx] = IpcEndpointOwner::EMPTY;
            return Ok(());
        }
        idx += 1;
    }
    Ok(())
}

pub(crate) fn endpoint_owner_pid(endpoint: u64) -> Option<u32> {
    ipc_endpoint_owner_pid(endpoint)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn force_release_named_endpoint(ep_name: &[u8]) {
    if let Some(ep) = crate::ipc::endpoint::lookup_endpoint(ep_name) {
        crate::ipc::channel::raw::mailbox_close(ep);
        let mut owners = IPC_ENDPOINT_OWNERS.lock();
        for slot in owners.iter_mut() {
            if slot.endpoint == ep.get() {
                *slot = IpcEndpointOwner::EMPTY;
            }
        }
    }
    let _ = crate::ipc::endpoint::unregister_endpoint(ep_name);
}

pub(super) fn ipc_endpoint_owner_pid(endpoint: u64) -> Option<u32> {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        return Some(packed_pid);
    }

    let owners = IPC_ENDPOINT_OWNERS.lock();
    owners
        .iter()
        .find(|entry| entry.endpoint == endpoint && entry.owner_pid != 0)
        .map(|entry| entry.owner_pid)
}

/// Returns `true` when `endpoint` is a well-known registered Ring 1 service
/// request mailbox (`(endpoint >> 32) == 0`) owned by a different process with
/// a recognized `ServiceClass` (never a raw Ring 3 client PID reply target).
pub(super) fn is_registered_service_request_mailbox(endpoint: u64, caller_pid: u32) -> bool {
    if endpoint == 0 || (endpoint >> 32) != 0 {
        return false;
    }
    let owner_pid = {
        let owners = IPC_ENDPOINT_OWNERS.lock();
        owners
            .iter()
            .find(|entry| entry.endpoint == endpoint && entry.owner_pid != 0)
            .map(|entry| entry.owner_pid)
    };
    match owner_pid {
        Some(owner) if owner != caller_pid => {
            crate::security::service_class_of(Pid(owner)) != crate::security::ServiceClass::Unknown
        }
        _ => false,
    }
}

pub(super) fn primary_ipc_endpoint_for_owner(owner_pid: u32) -> Option<u64> {
    if owner_pid == 0 {
        return None;
    }
    let owners = IPC_ENDPOINT_OWNERS.lock();
    owners
        .iter()
        .find(|entry| entry.owner_pid == owner_pid && entry.endpoint != 0)
        .map(|entry| entry.endpoint)
}

#[inline(always)]
pub(super) fn exo_ipc_endpoint_pid(endpoint: u64) -> u32 {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        packed_pid
    } else if let Some(owner_pid) = ipc_endpoint_owner_pid(endpoint) {
        owner_pid
    } else {
        endpoint as u32
    }
}

/// `exo_ipc_create(name_ptr, name_len, endpoint)` — opens server raw mailbox.
pub fn sys_exo_ipc_create(
    name_ptr: u64,
    name_len: u64,
    endpoint: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_CREATE);
    let len = name_len as usize;
    if len == 0 || len >= crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN {
        return EINVAL;
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };

    if let Err(err) = UserBuf::validate(name_ptr, len, crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN) {
        return err.to_errno();
    }
    let mut name_buf = [0u8; 128];
    let name = &mut name_buf[..len];
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    let reserved_class = service_class_for_endpoint_name(name);

    let is_system = if cfg!(test) {
        true
    } else if caller_pid == 1 {
        true
    } else {
        crate::process::core::registry::PROCESS_REGISTRY
            .find_by_pid(Pid(caller_pid))
            .map(|pcb| {
                let is_init_root = pcb.ppid().0 == 1 && pcb.creds.lock().euid == 0;
                if !is_init_root {
                    return false;
                }
                // FIX-03: Reserved Ring 1 / system service classes require canonical
                // binary identity attestation (prevents rogue root/init children like
                // test runners from hijacking reserved service classes).
                if let Some(class) = reserved_class {
                    pcb_name_matches_service_class(&pcb.name_snapshot(), class)
                } else {
                    true
                }
            })
            .unwrap_or(false)
    };

    if let Some(class) = reserved_class {
        let _ = class;
        if !is_system {
            return EPERM;
        }
        // FIX-03: Prevent a second live process from claiming an already-active
        // single-instance reserved service class (VirtioDriver is multi-driver).
        #[cfg(not(test))]
        if class != crate::security::ServiceClass::VirtioDriver
            && crate::security::ipc_policy::class_has_other_live_owner(class, caller_pid)
        {
            return EEXIST;
        }
    }

    // FIX-03: Never overwrite a live endpoint name owned by another live process.
    if let Some(existing_ep) = crate::ipc::endpoint::lookup_endpoint(name) {
        if let Some(existing_owner) = ipc_endpoint_owner_pid(existing_ep.get()) {
            if existing_owner != caller_pid && crate::process::is_alive(existing_owner) {
                return EEXIST;
            }
        }
    }

    let reserve_outcome = match reserve_ipc_endpoint_owner(endpoint, caller_pid, is_system) {
        Ok(outcome) => outcome,
        Err(errno) => return errno,
    };
    match reserve_outcome {
        ReserveEndpointOwnerOutcome::ReplacedDead => {
            crate::ipc::channel::raw::mailbox_close(ep);
            crate::ipc::endpoint::unregister_endpoint_by_id(ep);
        }
        ReserveEndpointOwnerOutcome::ReservedNew {
            evicted_dead_endpoint: Some(old_raw),
        } => {
            if let Some(old_ep) = EndpointId::new(old_raw) {
                crate::ipc::channel::raw::mailbox_close(old_ep);
                crate::ipc::endpoint::unregister_endpoint_by_id(old_ep);
            }
        }
        ReserveEndpointOwnerOutcome::ReservedNew {
            evicted_dead_endpoint: None,
        }
        | ReserveEndpointOwnerOutcome::AlreadyOwned => {}
    }

    let newly_owned = reserve_outcome != ReserveEndpointOwnerOutcome::AlreadyOwned;
    if crate::ipc::channel::raw::mailbox_open(ep) {
        if let Err(err) = crate::ipc::endpoint::register_endpoint(name, ep) {
            if newly_owned {
                crate::ipc::channel::raw::mailbox_close(ep);
                let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
            }
            return super::channel::ipc_error_to_errno(err);
        }
        if let Some(class) = service_class_for_endpoint_name(name) {
            let _ = crate::security::register_service_class(Pid(caller_pid), class);
        }
        if name == b"fb_server" {
            crate::arch::x86_64::framebuffer_early::deactivate_for_userspace();
        }
        0
    } else {
        if newly_owned {
            let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
        }
        ENOMEM
    }
}

/// `exo_ipc_destroy(endpoint)` — closes server raw mailbox.
pub fn sys_exo_ipc_destroy(endpoint: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_DESTROY);
    let endpoint_pid = exo_ipc_endpoint_pid(endpoint);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if endpoint_pid == 0 {
        return EINVAL;
    }
    if caller_pid == 0 || endpoint_pid != caller_pid {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    crate::ipc::channel::raw::mailbox_close(ep);
    crate::ipc::endpoint::unregister_endpoint_by_id(ep);
    let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
    0
}

/// `exo_ipc_lookup(name_ptr, name_len)` — resolves an IPC endpoint by name.
pub fn sys_exo_ipc_lookup(
    name_ptr: u64,
    name_len: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_LOOKUP);

    let len = name_len as usize;
    let max_len = crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN;
    if len == 0 || len >= max_len {
        return EINVAL;
    }
    if name_ptr == 0 {
        return EFAULT;
    }

    if let Err(err) = UserBuf::validate(name_ptr, len, max_len) {
        return err.to_errno();
    }

    let mut name_buf = [0u8; 128];
    let name = &mut name_buf[..len];
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    // S2.3: Block confined compat processes (`NO_RING1_CRITICAL_IPC`) from resolving
    // direct critical Ring 1 service endpoints (`device_server`, `crypto_server`,
    // `memory_server`, `init_server`, `exo_shield`).
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid != 0
        && (crate::security::zero_trust::process_restrictions(caller_pid)
            & crate::security::zero_trust::restriction_flags::NO_RING1_CRITICAL_IPC)
            != 0
    {
        let is_critical_name = matches!(
            service_class_for_endpoint_name(name),
            Some(
                crate::security::ServiceClass::InitServer
                    | crate::security::ServiceClass::CryptoServer
                    | crate::security::ServiceClass::MemoryServer
                    | crate::security::ServiceClass::DeviceServer
                    | crate::security::ServiceClass::ExoShield
            )
        ) || name == b"init_server";
        if is_critical_name {
            crate::security::exoledger::exo_ledger_append(
                crate::security::exoledger::ActionTag::IpcUnauthorized {
                    src_pid: caller_pid,
                    dst_pid: 1,
                },
            );
            crate::security::shield_feed::push_event(
                caller_pid,
                crate::security::shield_feed::event_type::IPC,
                crate::security::shield_feed::severity::HIGH,
                0xC047,
                1,
                0,
            );
            return EACCES;
        }
    }

    match crate::ipc::endpoint::lookup_endpoint(name) {
        Some(endpoint) => endpoint.get() as i64,
        None => ENOENT,
    }
}
