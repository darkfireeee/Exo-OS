#![no_std]
#![no_main]

//! # device_server — plan de contrôle GI-03
//!
//! Ce serveur coordonne le cycle de vie des drivers Ring 1 :
//! - registre PCI/topologie ;
//! - validation et émission des claims vers le noyau ;
//! - politiques power/reset ;
//! - journalisation hotplug et IOMMU côté userspace.

use core::panic::PanicInfo;

use spin::Mutex;

mod claim_validator;
mod hotplug;
mod iommu_service;
mod power;
mod protocol;
mod registry;

use claim_validator::validate_claim;
use hotplug::{DeviceEvent, DeviceEventKind, HotplugQueue};
use iommu_service::IommuLedger;
use power::{PowerPolicyTable, PowerState};
use protocol::{
    read_u32, read_u64, recv_request, register_endpoint, send_heartbeat, send_reply, DeviceReply,
    DeviceRequest, DEVICE_MSG_CLAIM, DEVICE_MSG_EVENT_POLL, DEVICE_MSG_FAULT, DEVICE_MSG_HEARTBEAT,
    DEVICE_MSG_POWER_SET, DEVICE_MSG_QUERY, DEVICE_MSG_REGISTER_DEVICE, DEVICE_MSG_RELEASE,
};
use registry::PciRegistry;

struct DeviceService {
    registry: PciRegistry,
    hotplug: HotplugQueue,
    iommu: IommuLedger,
    power: PowerPolicyTable,
}

impl DeviceService {
    const fn new() -> Self {
        Self {
            registry: PciRegistry::new(),
            hotplug: HotplugQueue::new(),
            iommu: IommuLedger::new(),
            power: PowerPolicyTable::new(),
        }
    }

    fn handle_register_device(&mut self, sender_pid: u32, payload: &[u8]) -> DeviceReply {
        if sender_pid != 1 {
            return DeviceReply::error(exo_syscall_abi::EPERM);
        }

        let phys_base = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let size = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let bdf_raw = match read_u32(payload, 16) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let parent_bdf_raw = match read_u32(payload, 20) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let vendor_device = match read_u32(payload, 24) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let class_code = match read_u32(payload, 28) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let flags = match read_u32(payload, 32) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };

        if flags & 0x1 != 0 {
            // SAFETY: appel admin GI-03 vers le noyau avec BDFs encodés.
            let rc = unsafe {
                exo_syscall_abi::syscall3(
                    exo_syscall_abi::SYS_PCI_SET_TOPOLOGY,
                    bdf_raw as u64,
                    parent_bdf_raw as u64,
                    1,
                )
            };
            if rc < 0 {
                return DeviceReply::error(rc);
            }
        }

        match self.registry.register_device(
            phys_base,
            size,
            bdf_raw,
            parent_bdf_raw,
            vendor_device,
            class_code,
            flags,
        ) {
            Ok(snapshot) => {
                self.hotplug.push(DeviceEvent::new(
                    DeviceEventKind::Registered,
                    snapshot.bdf_raw,
                    0,
                    snapshot.flags as u64,
                ));
                DeviceReply::ok(
                    snapshot.bdf_raw as u64,
                    snapshot.phys_base,
                    snapshot.size,
                    snapshot.flags,
                )
            }
            Err(err) => DeviceReply::error(err),
        }
    }

    fn handle_claim(&mut self, sender_pid: u32, payload: &[u8]) -> DeviceReply {
        if sender_pid != 1 {
            return DeviceReply::error(exo_syscall_abi::EPERM);
        }

        let phys_base = match read_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let size = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let owner_pid = match read_u32(payload, 16) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let bdf_raw = match read_u32(payload, 20) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let flags = match read_u32(payload, 24) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };

        if let Err(err) = validate_claim(&self.registry, phys_base, size, owner_pid, bdf_raw, flags)
        {
            return DeviceReply::error(err);
        }

        // SAFETY: l'appelant est l'admin Ring 1 ; le noyau attribue le claim au driver cible.
        let rc = unsafe {
            exo_syscall_abi::syscall5(
                exo_syscall_abi::SYS_PCI_CLAIM,
                phys_base,
                size,
                owner_pid as u64,
                bdf_raw as u64,
                (flags & 0x1 != 0) as u64,
            )
        };
        if rc < 0 {
            return DeviceReply::error(rc);
        }

        match self.registry.assign_owner(owner_pid, bdf_raw) {
            Ok(snapshot) => {
                self.iommu.bind_driver(owner_pid, bdf_raw);
                self.power.set_state(owner_pid, PowerState::Active);
                self.hotplug.push(DeviceEvent::new(
                    DeviceEventKind::Claimed,
                    bdf_raw,
                    owner_pid,
                    0,
                ));
                DeviceReply::ok(
                    snapshot.bdf_raw as u64,
                    snapshot.owner_pid as u64,
                    snapshot.phys_base,
                    snapshot.flags,
                )
            }
            Err(err) => DeviceReply::error(err),
        }
    }

    fn handle_release(&mut self, sender_pid: u32, payload: &[u8]) -> DeviceReply {
        if sender_pid != 1 {
            return DeviceReply::error(exo_syscall_abi::EPERM);
        }

        let driver_pid = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let signal = match read_u32(payload, 4) {
            Ok(0) => 15u32,
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };

        // SAFETY: envoi d'un signal standard au process driver pour déclencher le cleanup kernel.
        let rc = unsafe {
            exo_syscall_abi::syscall2(exo_syscall_abi::SYS_KILL, driver_pid as u64, signal as u64)
        };
        if rc < 0 {
            return DeviceReply::error(rc);
        }

        if let Some(snapshot) = self.registry.release_owner(driver_pid) {
            self.iommu.unbind_driver(driver_pid);
            self.power.note_release(driver_pid, signal);
            self.hotplug.push(DeviceEvent::new(
                DeviceEventKind::Released,
                snapshot.bdf_raw,
                driver_pid,
                signal as u64,
            ));
            DeviceReply::ok(
                snapshot.bdf_raw as u64,
                driver_pid as u64,
                signal as u64,
                snapshot.flags,
            )
        } else {
            DeviceReply::error(exo_syscall_abi::ENOENT)
        }
    }

    fn handle_fault(&mut self, payload: &[u8]) -> DeviceReply {
        let driver_pid = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let fault_code = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let value0 = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let value1 = match read_u64(payload, 16) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };

        self.iommu
            .report_fault(driver_pid, fault_code, value0, value1);
        let bdf_raw = self.registry.bdf_of_owner(driver_pid).unwrap_or(0);
        self.hotplug.push(DeviceEvent::new(
            DeviceEventKind::Faulted,
            bdf_raw,
            driver_pid,
            value0,
        ));
        DeviceReply::ok(driver_pid as u64, value0, value1, fault_code)
    }

    fn handle_event_poll(&mut self) -> DeviceReply {
        match self.hotplug.pop() {
            Some(event) => DeviceReply::ok(
                event.bdf_raw as u64,
                event.pid as u64,
                event.value,
                event.kind as u32,
            ),
            None => DeviceReply::error(exo_syscall_abi::EAGAIN),
        }
    }

    fn handle_power_set(&mut self, sender_pid: u32, payload: &[u8]) -> DeviceReply {
        if sender_pid != 1 {
            return DeviceReply::error(exo_syscall_abi::EPERM);
        }

        let driver_pid = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let state = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let value = match read_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };

        let state = match PowerState::from_u32(state) {
            Some(value_state) => value_state,
            None => return DeviceReply::error(exo_syscall_abi::EINVAL),
        };

        let snapshot = self.power.set_state(driver_pid, state);
        let bdf_raw = self.registry.bdf_of_owner(driver_pid).unwrap_or(0);
        self.hotplug.push(DeviceEvent::new(
            DeviceEventKind::PowerChanged,
            bdf_raw,
            driver_pid,
            value,
        ));
        DeviceReply::ok(
            driver_pid as u64,
            snapshot.state as u64,
            snapshot.restart_backoff_ms,
            snapshot.last_signal,
        )
    }

    fn handle_query(&mut self, payload: &[u8]) -> DeviceReply {
        let selector = match read_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };
        let mode = match read_u32(payload, 4) {
            Ok(value) => value,
            Err(err) => return DeviceReply::error(err),
        };

        if mode == 0 {
            match self.registry.snapshot_by_bdf(selector) {
                Some(snapshot) => DeviceReply::ok(
                    snapshot.bdf_raw as u64,
                    snapshot.owner_pid as u64,
                    snapshot.phys_base,
                    snapshot.class_code
                        ^ snapshot.vendor_device
                        ^ snapshot.parent_bdf_raw
                        ^ snapshot.flags,
                ),
                None => DeviceReply::error(exo_syscall_abi::ENOENT),
            }
        } else {
            match self.iommu.snapshot(selector) {
                Some(snapshot) => DeviceReply::ok(
                    snapshot.domain_hint as u64,
                    snapshot.last_fault_value0,
                    snapshot.last_fault_value1,
                    snapshot.fault_count ^ snapshot.last_fault_code,
                ),
                None => DeviceReply::error(exo_syscall_abi::ENOENT),
            }
        }
    }
}

static DEVICE_SERVICE: Mutex<DeviceService> = Mutex::new(DeviceService::new());

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = DeviceRequest::zeroed();

    loop {
        match recv_request(&mut request) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }

        let reply = if request.msg_type == DEVICE_MSG_HEARTBEAT {
            send_heartbeat()
        } else {
            dispatch(&request)
        };

        let _ = send_reply(request.sender_pid, &reply);
    }
}

fn dispatch(request: &DeviceRequest) -> DeviceReply {
    let mut service = DEVICE_SERVICE.lock();

    match request.msg_type {
        DEVICE_MSG_REGISTER_DEVICE => {
            service.handle_register_device(request.sender_pid, &request.payload)
        }
        DEVICE_MSG_CLAIM => service.handle_claim(request.sender_pid, &request.payload),
        DEVICE_MSG_RELEASE => service.handle_release(request.sender_pid, &request.payload),
        DEVICE_MSG_FAULT => service.handle_fault(&request.payload),
        DEVICE_MSG_EVENT_POLL => service.handle_event_poll(),
        DEVICE_MSG_POWER_SET => service.handle_power_set(request.sender_pid, &request.payload),
        DEVICE_MSG_QUERY => service.handle_query(&request.payload),
        _ => DeviceReply::error(exo_syscall_abi::EINVAL),
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        // SAFETY: panic terminale pour un serveur no_std monothread.
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
