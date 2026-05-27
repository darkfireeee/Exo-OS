use crate::process::core::pid::Pid;
use exo_types::{CapToken, CapabilityType, Rights as ServiceRights};
use spin::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcPolicyResult {
    Allowed,
    Denied,
    UnknownService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceClass {
    InitServer,
    IpcBroker,
    MemoryServer,
    VfsServer,
    CryptoServer,
    DeviceServer,
    NetworkServer,
    SchedulerServer,
    InputServer,
    TtyServer,
    Ps2Driver,
    VirtioDriver,
    ExoShield,
    Exosh,
    Unknown,
}

#[derive(Clone, Copy)]
struct ServiceEntry {
    pid: Pid,
    class: ServiceClass,
}

const INIT_SERVER_PID: u32 = 1;
const IPC_BROKER_PID: u32 = 2;
const MAX_REGISTERED_SERVICES: usize = 16;

static SERVICE_REGISTRY: RwLock<[Option<ServiceEntry>; MAX_REGISTERED_SERVICES]> =
    RwLock::new([None; MAX_REGISTERED_SERVICES]);

static POLICY: &[(ServiceClass, ServiceClass)] = &[
    (ServiceClass::InitServer, ServiceClass::MemoryServer),
    (ServiceClass::MemoryServer, ServiceClass::InitServer),
    (ServiceClass::InitServer, ServiceClass::VfsServer),
    (ServiceClass::VfsServer, ServiceClass::InitServer),
    (ServiceClass::InitServer, ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer, ServiceClass::InitServer),
    (ServiceClass::InitServer, ServiceClass::DeviceServer),
    (ServiceClass::DeviceServer, ServiceClass::InitServer),
    (ServiceClass::InitServer, ServiceClass::SchedulerServer),
    (ServiceClass::SchedulerServer, ServiceClass::InitServer),
    (ServiceClass::InitServer, ServiceClass::ExoShield),
    (ServiceClass::ExoShield, ServiceClass::InitServer),
    (ServiceClass::VfsServer, ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer, ServiceClass::VfsServer),
    (ServiceClass::VfsServer, ServiceClass::NetworkServer),
    (ServiceClass::NetworkServer, ServiceClass::VfsServer),
    (ServiceClass::DeviceServer, ServiceClass::VirtioDriver),
    (ServiceClass::VirtioDriver, ServiceClass::DeviceServer),
    (ServiceClass::DeviceServer, ServiceClass::InputServer),
    (ServiceClass::InputServer, ServiceClass::DeviceServer),
    (ServiceClass::DeviceServer, ServiceClass::Ps2Driver),
    (ServiceClass::Ps2Driver, ServiceClass::DeviceServer),
    (ServiceClass::Ps2Driver, ServiceClass::InputServer),
    (ServiceClass::InputServer, ServiceClass::Ps2Driver),
    (ServiceClass::InputServer, ServiceClass::TtyServer),
    (ServiceClass::TtyServer, ServiceClass::InputServer),
    (ServiceClass::TtyServer, ServiceClass::VfsServer),
    (ServiceClass::VfsServer, ServiceClass::TtyServer),
    (ServiceClass::ExoShield, ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer, ServiceClass::ExoShield),
    (ServiceClass::ExoShield, ServiceClass::InputServer),
    (ServiceClass::InputServer, ServiceClass::ExoShield),
    (ServiceClass::ExoShield, ServiceClass::TtyServer),
    (ServiceClass::TtyServer, ServiceClass::ExoShield),
    (ServiceClass::Exosh, ServiceClass::IpcBroker),
    (ServiceClass::Exosh, ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer, ServiceClass::Exosh),
    (ServiceClass::Exosh, ServiceClass::InputServer),
    (ServiceClass::InputServer, ServiceClass::Exosh),
    (ServiceClass::Exosh, ServiceClass::TtyServer),
    (ServiceClass::TtyServer, ServiceClass::Exosh),
    (ServiceClass::Exosh, ServiceClass::ExoShield),
    (ServiceClass::ExoShield, ServiceClass::Exosh),
    (ServiceClass::NetworkServer, ServiceClass::DeviceServer),
    (ServiceClass::DeviceServer, ServiceClass::NetworkServer),
    (ServiceClass::NetworkServer, ServiceClass::VirtioDriver),
    (ServiceClass::VirtioDriver, ServiceClass::NetworkServer),
];

const _: () = assert!(
    POLICY.len() == 47,
    "IPC policy Ring 1 doit rester synchronisée avec Architecture v7"
);

#[inline]
fn class_from_capability(cap_type: CapabilityType) -> ServiceClass {
    match cap_type {
        CapabilityType::IpcBroker => ServiceClass::IpcBroker,
        CapabilityType::MemoryServer => ServiceClass::MemoryServer,
        CapabilityType::DriverPci => ServiceClass::VirtioDriver,
        CapabilityType::SysDeviceAdmin => ServiceClass::DeviceServer,
        CapabilityType::ExoFsAccess | CapabilityType::VfsServer => ServiceClass::VfsServer,
        CapabilityType::CryptoServer => ServiceClass::CryptoServer,
        CapabilityType::ExoPhoenix => ServiceClass::ExoShield,
        CapabilityType::SchedulerServer => ServiceClass::SchedulerServer,
    }
}

#[inline]
fn is_ring1_trusted_class(class: ServiceClass) -> bool {
    matches!(
        class,
        ServiceClass::InitServer
            | ServiceClass::IpcBroker
            | ServiceClass::MemoryServer
            | ServiceClass::VfsServer
            | ServiceClass::CryptoServer
            | ServiceClass::DeviceServer
            | ServiceClass::NetworkServer
            | ServiceClass::SchedulerServer
            | ServiceClass::InputServer
            | ServiceClass::TtyServer
            | ServiceClass::Ps2Driver
            | ServiceClass::VirtioDriver
            | ServiceClass::ExoShield
    )
}

pub fn register_service(pid: Pid, cap: &CapToken) -> bool {
    if cap.generation == 0
        || cap._pad != [0; 2]
        || !cap.object_id.is_valid()
        || cap.rights == ServiceRights::NONE.0
        || (cap.rights & !ServiceRights::ALL.0) != 0
    {
        return false;
    }

    let Some(cap_type) = CapabilityType::from_u16(cap.type_id) else {
        return false;
    };
    register_service_class(pid, class_from_capability(cap_type))
}

pub fn register_service_class(pid: Pid, class: ServiceClass) -> bool {
    if pid.0 == INIT_SERVER_PID
        || pid.0 == IPC_BROKER_PID
        || class == ServiceClass::InitServer
        || class == ServiceClass::IpcBroker
        || class == ServiceClass::Unknown
    {
        return false;
    }

    let mut reg = SERVICE_REGISTRY.write();
    for slot in reg.iter_mut() {
        if let Some(entry) = slot {
            if entry.pid == pid {
                if is_ring1_trusted_class(entry.class) && !is_ring1_trusted_class(class) {
                    crate::security::zero_trust::unregister_ring1_pid(pid.0);
                }
                if is_ring1_trusted_class(class) {
                    crate::security::zero_trust::register_ring1_pid(pid.0);
                }
                entry.class = class;
                return true;
            }
        }
    }
    for slot in reg.iter_mut() {
        if slot.is_none() {
            *slot = Some(ServiceEntry { pid, class });
            if is_ring1_trusted_class(class) {
                crate::security::zero_trust::register_ring1_pid(pid.0);
            }
            return true;
        }
    }
    false
}

pub fn unregister_service(pid: Pid) -> bool {
    let mut reg = SERVICE_REGISTRY.write();
    for slot in reg.iter_mut() {
        if let Some(entry) = *slot {
            if entry.pid != pid {
                continue;
            }
            if is_ring1_trusted_class(entry.class) {
                crate::security::zero_trust::unregister_ring1_pid(pid.0);
            }
            *slot = None;
            return true;
        }
    }
    false
}

fn class_of(pid: Pid) -> ServiceClass {
    match pid.0 {
        INIT_SERVER_PID => ServiceClass::InitServer,
        IPC_BROKER_PID => ServiceClass::IpcBroker,
        _ => {
            let reg = SERVICE_REGISTRY.read();
            reg.iter()
                .flatten()
                .find(|entry| entry.pid == pid)
                .map(|entry| entry.class)
                .unwrap_or(ServiceClass::Unknown)
        }
    }
}

pub fn service_class_of(pid: Pid) -> ServiceClass {
    class_of(pid)
}

pub fn can_inject_src_pid(pid: Pid) -> bool {
    is_ring1_trusted_class(class_of(pid))
}

pub fn check_direct_ipc(src: Pid, dst: Pid) -> IpcPolicyResult {
    let src_class = class_of(src);
    let dst_class = class_of(dst);

    if src_class == ServiceClass::Unknown || dst_class == ServiceClass::Unknown {
        return IpcPolicyResult::UnknownService;
    }

    if src_class == ServiceClass::IpcBroker {
        return IpcPolicyResult::Allowed;
    }

    if POLICY
        .iter()
        .any(|&(allowed_src, allowed_dst)| allowed_src == src_class && allowed_dst == dst_class)
    {
        IpcPolicyResult::Allowed
    } else {
        IpcPolicyResult::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_service(pid: u32, class: ServiceClass) -> Pid {
        let pid = Pid(pid);
        let _ = register_service_class(pid, class);
        pid
    }

    #[test]
    fn ipc_broker_is_allowed_to_all_registered_services() {
        let shield = with_service(42, ServiceClass::ExoShield);
        assert_eq!(
            check_direct_ipc(Pid(IPC_BROKER_PID), shield),
            IpcPolicyResult::Allowed
        );
        let _ = unregister_service(shield);
    }

    #[test]
    fn unauthorized_direct_path_is_denied() {
        let network = with_service(43, ServiceClass::NetworkServer);
        let crypto = with_service(44, ServiceClass::CryptoServer);
        assert_eq!(check_direct_ipc(network, crypto), IpcPolicyResult::Denied);
        let _ = unregister_service(network);
        let _ = unregister_service(crypto);
    }

    #[test]
    fn init_has_required_request_reply_paths() {
        let crypto = with_service(45, ServiceClass::CryptoServer);
        let device = with_service(46, ServiceClass::DeviceServer);
        let scheduler = with_service(47, ServiceClass::SchedulerServer);

        assert_eq!(
            check_direct_ipc(Pid(INIT_SERVER_PID), crypto),
            IpcPolicyResult::Allowed
        );
        assert_eq!(
            check_direct_ipc(crypto, Pid(INIT_SERVER_PID)),
            IpcPolicyResult::Allowed
        );
        assert_eq!(
            check_direct_ipc(Pid(INIT_SERVER_PID), device),
            IpcPolicyResult::Allowed
        );
        assert_eq!(
            check_direct_ipc(scheduler, Pid(INIT_SERVER_PID)),
            IpcPolicyResult::Allowed
        );

        let _ = unregister_service(crypto);
        let _ = unregister_service(device);
        let _ = unregister_service(scheduler);
    }

    #[test]
    fn network_server_has_bidirectional_driver_control_path() {
        let network = with_service(54, ServiceClass::NetworkServer);
        let driver = with_service(55, ServiceClass::VirtioDriver);

        assert_eq!(check_direct_ipc(network, driver), IpcPolicyResult::Allowed);
        assert_eq!(check_direct_ipc(driver, network), IpcPolicyResult::Allowed);

        let _ = unregister_service(network);
        let _ = unregister_service(driver);
    }

    #[test]
    fn unknown_dynamic_pid_is_not_treated_as_a_service() {
        assert_eq!(
            check_direct_ipc(Pid(INIT_SERVER_PID), Pid(9000)),
            IpcPolicyResult::UnknownService
        );
        assert!(!can_inject_src_pid(Pid(9000)));
    }

    #[test]
    fn only_ring1_services_can_request_kernel_pid_injection() {
        let network = with_service(52, ServiceClass::NetworkServer);
        let shell = with_service(53, ServiceClass::Exosh);

        assert!(can_inject_src_pid(Pid(INIT_SERVER_PID)));
        assert!(can_inject_src_pid(network));
        assert!(!can_inject_src_pid(shell));

        let _ = unregister_service(network);
        let _ = unregister_service(shell);
    }

    #[test]
    fn dynamic_pid_cannot_claim_ipc_broker_class() {
        let broker_alias = Pid(48);
        assert!(!register_service_class(
            broker_alias,
            ServiceClass::IpcBroker
        ));
        assert_eq!(
            check_direct_ipc(broker_alias, Pid(INIT_SERVER_PID)),
            IpcPolicyResult::UnknownService
        );
    }

    #[test]
    fn revoked_service_cap_cannot_register_dynamic_service() {
        let revoked_cap = CapToken {
            generation: 0,
            object_id: exo_types::ObjectId::from_counter(49),
            rights: exo_types::Rights::READ.0,
            type_id: CapabilityType::CryptoServer as u16,
            _pad: [0; 2],
        };

        assert!(!register_service(Pid(49), &revoked_cap));
        assert_eq!(
            check_direct_ipc(Pid(INIT_SERVER_PID), Pid(49)),
            IpcPolicyResult::UnknownService
        );
    }

    #[test]
    fn malformed_service_cap_cannot_register_dynamic_service() {
        let zero_object_cap = CapToken {
            generation: 1,
            object_id: exo_types::ObjectId::ZERO,
            rights: exo_types::Rights::READ.0,
            type_id: CapabilityType::CryptoServer as u16,
            _pad: [0; 2],
        };
        assert!(!register_service(Pid(50), &zero_object_cap));

        let bad_rights_cap = CapToken {
            generation: 1,
            object_id: exo_types::ObjectId::from_counter(50),
            rights: exo_types::Rights::ALL.0 | 0x8000_0000,
            type_id: CapabilityType::CryptoServer as u16,
            _pad: [0; 2],
        };
        assert!(!register_service(Pid(51), &bad_rights_cap));
    }
}
