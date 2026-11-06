// kernel/src/ipc/capability_bridge/mod.rs
//
// Compatibility IPC -> security::access_control.
// IPC keeps this module as a stable facade, but actual verification goes
// through the unified security/access_control checker.

pub mod check;

pub use check::{
    capability_to_ipc_error, check_channel_access, check_endpoint_access, check_shm_access,
};
