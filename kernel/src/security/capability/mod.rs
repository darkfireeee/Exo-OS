//! # security/capability — Capability Subsystem Root (Exo-OS Layer 2b)
//!
//! RULE CAP-01: `security/capability/` is the unique source of truth for capabilities.
//! Implementation of kernel/service capability issuance and syscall wrappers lives
//! in `service_tokens.rs`.

pub mod delegation;
pub mod namespace;
pub mod revocation;
pub mod rights;
pub mod service_tokens;
pub mod table;
pub mod token;
pub mod verify;
#[cfg(test)]
mod tests;

pub use delegation::{
    can_delegate, delegate, delegate_all, delegate_read_only, DelegationChain, DelegationEntry,
};
pub use namespace::{alloc_namespace_id, cross_namespace_verify, CapNamespace, NamespaceId};
pub use revocation::{revoke, revoke_token};
pub use rights::Rights;
pub use service_tokens::{
    capget, capset, check_token, check_token_owner, create, init_capability_subsystem,
    is_initialized, mint_kernel_ipc_token, revoke_handle, revoke_handle_by_caller, KernelCapError,
};
#[cfg(feature = "phoenix-fault-injection")]
pub use service_tokens::{mint_phoenix_fault_inject_token, verify_phoenix_fault_inject_token};
pub use table::{CapTable, CapTableSnapshot, CAP_TABLE_CAPACITY};
pub use token::{
    read_stats as token_stats, CapObjectType, CapToken, ObjectId, TokenStats, CAP_TOKEN_WIRE_SIZE,
};
pub use verify::{
    verify, verify_and_get_rights, verify_ipc_recv, verify_ipc_send, verify_read,
    verify_read_write, verify_typed, CapError,
};
