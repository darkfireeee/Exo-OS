// kernel/src/ipc/capability_bridge/mod.rs
//
// Compatibilité IPC -> security::access_control.
// IPC garde ce module comme façade stable, mais la vérification réelle passe
// par le checker unifié security/access_control.

pub mod check;

pub use check::{
    capability_to_ipc_error, check_channel_access, check_endpoint_access, check_shm_access,
};
