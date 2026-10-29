//! # syscall/table/ipc/endpoints.rs — Gestion et enregistrement des endpoints IPC et mailboxes

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
        b"tty_server" => Some(crate::security::ServiceClass::TtyServer),
        b"fb_server" => Some(crate::security::ServiceClass::FbServer),
        b"exo_shield" => Some(crate::security::ServiceClass::ExoShield),
        b"exosh" => Some(crate::security::ServiceClass::Exosh),
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

pub(super) fn reserve_ipc_endpoint_owner(endpoint: u64, owner_pid: u32) -> Result<bool, i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut empty_slot = None;
    let mut idx = 0usize;
    while idx < owners.len() {
        let entry = owners[idx];
        if entry.endpoint == endpoint {
            if entry.owner_pid == owner_pid {
                return Ok(false);
            }
            if !crate::process::is_alive(entry.owner_pid) {
                owners[idx] = IpcEndpointOwner {
                    endpoint,
                    owner_pid,
                };
                return Ok(true);
            }
            return Err(EBUSY);
        }
        if entry.endpoint == 0 && empty_slot.is_none() {
            empty_slot = Some(idx);
        }
        idx += 1;
    }

    let Some(slot) = empty_slot else {
        return Err(ENOMEM);
    };
    owners[slot] = IpcEndpointOwner {
        endpoint,
        owner_pid,
    };
    Ok(false)
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

/// `exo_ipc_create(name_ptr, name_len, endpoint)` — ouvre la mailbox raw du serveur.
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
    if len == 0 || len > 128 {
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

    if let Err(err) = UserBuf::validate(name_ptr, len, 128) {
        return err.to_errno();
    }
    let mut name_buf = [0u8; 128];
    let name = &mut name_buf[..len];
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    let replaced_dead_owner = match reserve_ipc_endpoint_owner(endpoint, caller_pid) {
        Ok(replaced) => replaced,
        Err(errno) => return errno,
    };
    if replaced_dead_owner {
        crate::ipc::channel::raw::mailbox_close(ep);
        crate::ipc::endpoint::unregister_endpoint_by_id(ep);
    }

    if crate::ipc::channel::raw::mailbox_open(ep) {
        if let Err(err) = crate::ipc::endpoint::register_endpoint(name, ep) {
            crate::ipc::channel::raw::mailbox_close(ep);
            let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
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
        let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
        ENOMEM
    }
}

/// `exo_ipc_destroy(endpoint)` — ferme la mailbox raw du serveur.
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

/// `exo_ipc_lookup(name_ptr, name_len)` — résout un endpoint IPC par nom.
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
    let max_len = crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN.min(128);
    if len == 0 || len > max_len {
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

    match crate::ipc::endpoint::lookup_endpoint(name) {
        Some(endpoint) => endpoint.get() as i64,
        None => ENOENT,
    }
}
