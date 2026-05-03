// kernel/src/ipc/capability_bridge/check.rs

use crate::ipc::core::types::IpcError;
use crate::security::access_control::{check_access, AccessError, ObjectKind};
use crate::security::capability::{CapTable, CapToken, Rights};

#[inline]
pub fn capability_to_ipc_error(err: AccessError) -> IpcError {
    match err {
        AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
        AccessError::InsufficientRights { .. } | AccessError::CapabilityDenied { .. } => {
            IpcError::PermissionDenied
        }
    }
}

#[inline]
pub fn check_endpoint_access(
    table: &CapTable,
    token: CapToken,
    rights: Rights,
) -> Result<(), IpcError> {
    check_access(table, token, ObjectKind::IpcEndpoint, rights, "ipc")
        .map_err(capability_to_ipc_error)
}

#[inline]
pub fn check_channel_access(
    table: &CapTable,
    token: CapToken,
    rights: Rights,
) -> Result<(), IpcError> {
    check_access(table, token, ObjectKind::IpcChannel, rights, "ipc")
        .map_err(capability_to_ipc_error)
}

#[inline]
pub fn check_shm_access(table: &CapTable, token: CapToken, rights: Rights) -> Result<(), IpcError> {
    check_access(table, token, ObjectKind::ShmRegion, rights, "ipc")
        .map_err(capability_to_ipc_error)
}
