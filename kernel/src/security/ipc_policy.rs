use crate::process::core::pid::Pid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcPolicyResult {
    Allowed,
    Denied,
    UnknownService,
}

const INIT_SERVER_PID: u32 = 1;
const IPC_ROUTER_PID: u32 = 2;
const MEMORY_SERVER_PID: u32 = 3;
const VFS_SERVER_PID: u32 = 4;
const CRYPTO_SERVER_PID: u32 = 5;
const DEVICE_SERVER_PID: u32 = 6;
const NETWORK_SERVER_PID: u32 = 7;
const SCHEDULER_SERVER_PID: u32 = 8;
const VIRTIO_DRIVERS_PID: u32 = 9;
const EXO_SHIELD_PID: u32 = 10;

// Cette table reflète la topologie Ring1 réellement présente dans init_server.
static KERNEL_IPC_POLICY: &[(u32, u32)] = &[
    (INIT_SERVER_PID, MEMORY_SERVER_PID),
    (INIT_SERVER_PID, VFS_SERVER_PID),
    (VFS_SERVER_PID, CRYPTO_SERVER_PID),
    (NETWORK_SERVER_PID, VFS_SERVER_PID),
    (DEVICE_SERVER_PID, VIRTIO_DRIVERS_PID),
];

const _: () = assert!(
    KERNEL_IPC_POLICY.len() == 5,
    "KERNEL_IPC_POLICY doit rester synchronisé avec exocordon.rs"
);

#[inline(always)]
fn is_known_service(pid: u32) -> bool {
    matches!(
        pid,
        INIT_SERVER_PID
            | IPC_ROUTER_PID
            | MEMORY_SERVER_PID
            | VFS_SERVER_PID
            | CRYPTO_SERVER_PID
            | DEVICE_SERVER_PID
            | NETWORK_SERVER_PID
            | SCHEDULER_SERVER_PID
            | VIRTIO_DRIVERS_PID
            | EXO_SHIELD_PID
    )
}

pub fn check_direct_ipc(src: Pid, dst: Pid) -> IpcPolicyResult {
    let src_raw = src.0;
    let dst_raw = dst.0;

    if !is_known_service(src_raw) || !is_known_service(dst_raw) {
        return IpcPolicyResult::UnknownService;
    }

    // ipc_router reste le courtier autorisé à router vers les autres services.
    if src_raw == IPC_ROUTER_PID {
        return IpcPolicyResult::Allowed;
    }

    if KERNEL_IPC_POLICY
        .iter()
        .any(|&(allowed_src, allowed_dst)| allowed_src == src_raw && allowed_dst == dst_raw)
    {
        IpcPolicyResult::Allowed
    } else {
        IpcPolicyResult::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_router_is_allowed_to_all_known_services() {
        assert_eq!(
            check_direct_ipc(Pid(IPC_ROUTER_PID), Pid(EXO_SHIELD_PID)),
            IpcPolicyResult::Allowed
        );
    }

    #[test]
    fn unauthorized_direct_path_is_denied() {
        assert_eq!(
            check_direct_ipc(Pid(NETWORK_SERVER_PID), Pid(CRYPTO_SERVER_PID)),
            IpcPolicyResult::Denied
        );
    }
}
