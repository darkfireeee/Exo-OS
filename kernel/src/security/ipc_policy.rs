use crate::process::core::pid::Pid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcPolicyResult {
    Allowed,
    Denied,
    UnknownService,
}

const INIT_SERVER_PID: u32 = 1;
const IPC_ROUTER_PID: u32 = 2;
const VFS_SERVER_PID: u32 = 3;
const CRYPTO_SERVER_PID: u32 = 4;
const MEMORY_SERVER_PID: u32 = 5;
const DEVICE_SERVER_PID: u32 = 6;
const NETWORK_SERVER_PID: u32 = 7;
const SCHEDULER_SERVER_PID: u32 = 8;
const VIRTIO_DRIVERS_PID: u32 = 9;
const EXO_SHIELD_PID: u32 = 10;

// Cette table reflète les endpoints Ring1 réellement enregistrés.
// Les flux requête/réponse directs utilisent SYS_IPC_SEND des deux côtés :
// les arêtes retour sont donc explicites, sinon les réponses seraient bloquées.
static KERNEL_IPC_POLICY: &[(u32, u32)] = &[
    (INIT_SERVER_PID, MEMORY_SERVER_PID),
    (MEMORY_SERVER_PID, INIT_SERVER_PID),
    (INIT_SERVER_PID, VFS_SERVER_PID),
    (VFS_SERVER_PID, INIT_SERVER_PID),
    (INIT_SERVER_PID, EXO_SHIELD_PID),
    (EXO_SHIELD_PID, INIT_SERVER_PID),
    (VFS_SERVER_PID, CRYPTO_SERVER_PID),
    (CRYPTO_SERVER_PID, VFS_SERVER_PID),
    (NETWORK_SERVER_PID, VFS_SERVER_PID),
    (VFS_SERVER_PID, NETWORK_SERVER_PID),
    (DEVICE_SERVER_PID, VIRTIO_DRIVERS_PID),
    (VIRTIO_DRIVERS_PID, DEVICE_SERVER_PID),
    (EXO_SHIELD_PID, CRYPTO_SERVER_PID),
    (CRYPTO_SERVER_PID, EXO_SHIELD_PID),
];

const _: () = assert!(
    KERNEL_IPC_POLICY.len() == 14,
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

    #[test]
    fn exo_shield_crypto_request_reply_paths_are_allowed() {
        assert_eq!(
            check_direct_ipc(Pid(EXO_SHIELD_PID), Pid(CRYPTO_SERVER_PID)),
            IpcPolicyResult::Allowed
        );
        assert_eq!(
            check_direct_ipc(Pid(CRYPTO_SERVER_PID), Pid(EXO_SHIELD_PID)),
            IpcPolicyResult::Allowed
        );
    }

    #[test]
    fn init_exo_shield_request_reply_paths_are_allowed() {
        assert_eq!(
            check_direct_ipc(Pid(INIT_SERVER_PID), Pid(EXO_SHIELD_PID)),
            IpcPolicyResult::Allowed
        );
        assert_eq!(
            check_direct_ipc(Pid(EXO_SHIELD_PID), Pid(INIT_SERVER_PID)),
            IpcPolicyResult::Allowed
        );
    }
}
