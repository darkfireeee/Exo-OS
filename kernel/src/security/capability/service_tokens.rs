//! Kernel capability table, Ring 1 IPC service token registry, and syscall wrappers.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::scheduler::sync::spinlock::SpinLock;

use super::{revocation, rights::Rights, table, token, verify};

static CAP_INIT_DONE: AtomicBool = AtomicBool::new(false);
static KERNEL_CAP_TABLE: SpinLock<Option<table::CapTable>> = SpinLock::new(None);

const SERVICE_CAP_META_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServiceCapMeta {
    object_id: token::ObjectId,
    owner_pid: u32,
    target_pid: u32,
    type_tag: token::CapObjectType,
}

impl ServiceCapMeta {
    const fn empty() -> Self {
        Self {
            object_id: token::ObjectId::INVALID,
            owner_pid: 0,
            target_pid: 0,
            type_tag: token::CapObjectType::Invalid,
        }
    }

    #[inline(always)]
    fn is_free(self) -> bool {
        self.object_id == token::ObjectId::INVALID
    }
}

static SERVICE_CAP_META: SpinLock<[ServiceCapMeta; SERVICE_CAP_META_CAPACITY]> =
    SpinLock::new([ServiceCapMeta::empty(); SERVICE_CAP_META_CAPACITY]);

static OBJ_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[inline]
fn alloc_object_id() -> token::ObjectId {
    token::ObjectId::from_raw(OBJ_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn insert_service_cap_meta(
    object_id: token::ObjectId,
    owner_pid: u32,
    target_pid: u32,
    type_tag: token::CapObjectType,
) -> Result<(), KernelCapError> {
    let mut metas = SERVICE_CAP_META.lock();
    for slot in metas.iter_mut() {
        if slot.is_free() {
            *slot = ServiceCapMeta {
                object_id,
                owner_pid,
                target_pid,
                type_tag,
            };
            return Ok(());
        }
    }
    Err(KernelCapError::InvalidArg)
}

fn lookup_service_cap_meta(object_id: token::ObjectId) -> Option<ServiceCapMeta> {
    let metas = SERVICE_CAP_META.lock();
    for slot in metas.iter() {
        if slot.object_id == object_id {
            return Some(*slot);
        }
    }
    None
}

fn remove_service_cap_meta(object_id: token::ObjectId) {
    let mut metas = SERVICE_CAP_META.lock();
    for slot in metas.iter_mut() {
        if slot.object_id == object_id {
            *slot = ServiceCapMeta::empty();
            return;
        }
    }
}

/// Initializes the capability subsystem (idempotent).
pub fn init_capability_subsystem() {
    if CAP_INIT_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    *KERNEL_CAP_TABLE.lock() = Some(table::CapTable::new());
}

/// Returns true if the subsystem is initialized.
#[inline(always)]
pub fn is_initialized() -> bool {
    CAP_INIT_DONE.load(Ordering::Acquire)
}

/// Error returned by capability syscall wrappers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCapError {
    NotFound,
    PermissionDenied,
    InvalidArg,
    NotSupported,
    Expired,
}

impl KernelCapError {
    #[inline]
    pub const fn to_kernel_errno(self) -> i32 {
        match self {
            Self::NotFound => -2,         // ENOENT
            Self::PermissionDenied => -1, // EPERM
            Self::InvalidArg => -22,      // EINVAL
            Self::NotSupported => -38,    // ENOSYS
            Self::Expired => -110,        // ETIMEDOUT
        }
    }
}

/// Creates an IPC service capability for a target endpoint.
pub fn create(
    cap_type: u32,
    rights: u32,
    target_pid: u32,
    owner_pid: u32,
) -> Result<token::CapToken, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    if owner_pid == 0 || target_pid == 0 {
        return Err(KernelCapError::InvalidArg);
    }
    let rights_val = Rights::from_bits(rights).ok_or(KernelCapError::InvalidArg)?;
    if cap_type > u16::MAX as u32 {
        return Err(KernelCapError::InvalidArg);
    }
    let obj_type = token::CapObjectType::from_u16(cap_type as u16);
    if obj_type == token::CapObjectType::Invalid {
        return Err(KernelCapError::InvalidArg);
    }
    if obj_type != token::CapObjectType::IpcEndpoint {
        return Err(KernelCapError::NotSupported);
    }
    if !rights_val.contains(Rights::IPC_SEND) {
        return Err(KernelCapError::InvalidArg);
    }
    if rights_val.contains(Rights::IPC_MANAGE) {
        return Err(KernelCapError::PermissionDenied);
    }

    let verdict = crate::security::check_direct_ipc(
        crate::process::core::pid::Pid(owner_pid),
        crate::process::core::pid::Pid(target_pid),
    );
    match verdict {
        crate::security::IpcPolicyResult::Allowed => {}
        crate::security::IpcPolicyResult::Denied
        | crate::security::IpcPolicyResult::UnknownService => {
            return Err(KernelCapError::PermissionDenied);
        }
    }

    let oid = alloc_object_id();
    let mut guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_mut().ok_or(KernelCapError::NotSupported)?;
    let token = table
        .grant(oid, rights_val, obj_type)
        .map_err(|_| KernelCapError::InvalidArg)?;
    drop(guard);

    if let Err(err) = insert_service_cap_meta(oid, owner_pid, target_pid, obj_type) {
        let guard = KERNEL_CAP_TABLE.lock();
        if let Some(table) = guard.as_ref() {
            revocation::revoke(table, oid);
        }
        return Err(err);
    }

    crate::security::exokairos::register_ttl_for_cap(oid, rights_val);
    Ok(token)
}

/// Mints a Ring 0 kernel-authenticated IPC capability token (`T20-001`).
pub fn mint_kernel_ipc_token(
    target_pid: u32,
    rights: Rights,
) -> Result<token::CapToken, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    if target_pid == 0 {
        return Err(KernelCapError::InvalidArg);
    }

    let oid = alloc_object_id();
    let mut guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_mut().ok_or(KernelCapError::NotSupported)?;
    let token = table
        .grant(oid, rights, token::CapObjectType::IpcEndpoint)
        .map_err(|_| KernelCapError::InvalidArg)?;
    drop(guard);

    if let Err(err) = insert_service_cap_meta(oid, 0, target_pid, token::CapObjectType::IpcEndpoint)
    {
        let guard = KERNEL_CAP_TABLE.lock();
        if let Some(table) = guard.as_ref() {
            revocation::revoke(table, oid);
        }
        return Err(err);
    }

    Ok(token)
}

#[cfg(feature = "phoenix-fault-injection")]
pub fn mint_phoenix_fault_inject_token(owner_pid: u32) -> Result<token::CapToken, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    if owner_pid == 0 {
        return Err(KernelCapError::InvalidArg);
    }
    let rights = Rights::IPC_SEND | Rights::IPC_MANAGE;
    let oid = alloc_object_id();
    let mut guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_mut().ok_or(KernelCapError::NotSupported)?;
    let token = table
        .grant(oid, rights, token::CapObjectType::IpcEndpoint)
        .map_err(|_| KernelCapError::InvalidArg)?;
    drop(guard);

    if let Err(err) =
        insert_service_cap_meta(oid, owner_pid, owner_pid, token::CapObjectType::IpcEndpoint)
    {
        let guard = KERNEL_CAP_TABLE.lock();
        if let Some(table) = guard.as_ref() {
            revocation::revoke(table, oid);
        }
        return Err(err);
    }
    Ok(token)
}

#[cfg(feature = "phoenix-fault-injection")]
pub fn verify_phoenix_fault_inject_token(
    token: token::CapToken,
    caller_pid: u32,
) -> Result<(), KernelCapError> {
    let required = (Rights::IPC_SEND | Rights::IPC_MANAGE).bits();
    let _ = check_token_owner(
        token,
        required,
        caller_pid,
        caller_pid,
        token::CapObjectType::IpcEndpoint as u32,
    )?;
    Ok(())
}

/// Verifies a serialized IPC service token issued via `create()`.
pub fn check_token(
    token: token::CapToken,
    required_rights: u32,
    expected_target_pid: u32,
    expected_type: u32,
) -> Result<token::ObjectId, KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    if expected_target_pid == 0 || expected_type > u16::MAX as u32 {
        return Err(KernelCapError::InvalidArg);
    }

    let rights = Rights::from_bits(required_rights).ok_or(KernelCapError::InvalidArg)?;
    let expected_type = token::CapObjectType::from_u16(expected_type as u16);
    if expected_type == token::CapObjectType::Invalid {
        return Err(KernelCapError::InvalidArg);
    }

    let guard = KERNEL_CAP_TABLE.lock();
    let table = guard.as_ref().ok_or(KernelCapError::NotSupported)?;
    verify::verify_typed(table, token, rights, expected_type)
        .map_err(|_| KernelCapError::PermissionDenied)?;
    drop(guard);

    let meta = lookup_service_cap_meta(token.object_id()).ok_or(KernelCapError::NotFound)?;
    if meta.target_pid != expected_target_pid || meta.type_tag != expected_type {
        return Err(KernelCapError::PermissionDenied);
    }

    // FIX-04: Enforce ExoKairos constant-time temporal TTL on capability checks.
    if crate::security::exokairos::verify_cap_deadline(meta.object_id).is_err() {
        crate::security::exoledger::exo_ledger_append(
            crate::security::exoledger::ActionTag::AccessDenied {
                oid: meta.object_id.as_u64(),
                rights: required_rights,
            },
        );
        crate::security::shield_feed::push_event(
            meta.owner_pid,
            crate::security::shield_feed::event_type::CAPABILITY,
            crate::security::shield_feed::severity::HIGH,
            0x4B41,
            meta.object_id.as_u64(),
            expected_target_pid as u64,
        );
        return Err(KernelCapError::Expired);
    }

    Ok(meta.object_id)
}

/// Verifies an IPC service token and its expected owner.
pub fn check_token_owner(
    token: token::CapToken,
    required_rights: u32,
    expected_owner_pid: u32,
    expected_target_pid: u32,
    expected_type: u32,
) -> Result<token::ObjectId, KernelCapError> {
    if expected_owner_pid == 0 {
        return Err(KernelCapError::InvalidArg);
    }
    let object_id = check_token(token, required_rights, expected_target_pid, expected_type)?;
    let meta = lookup_service_cap_meta(token.object_id()).ok_or(KernelCapError::NotFound)?;
    if meta.owner_pid != expected_owner_pid {
        return Err(KernelCapError::PermissionDenied);
    }
    Ok(object_id)
}

/// Revokes a capability by opaque handle.
pub fn revoke_handle(handle: u32) -> Result<(), KernelCapError> {
    revoke_handle_by_caller(0, handle)
}

/// Revokes a capability with verification of caller identity.
pub fn revoke_handle_by_caller(caller_pid: u32, handle: u32) -> Result<(), KernelCapError> {
    if !is_initialized() {
        return Err(KernelCapError::NotSupported);
    }
    let object_id = token::ObjectId::from_raw(handle as u64);

    if caller_pid != 0 && caller_pid != 1 {
        match lookup_service_cap_meta(object_id) {
            Some(meta) => {
                if meta.owner_pid != caller_pid && meta.target_pid != caller_pid {
                    return Err(KernelCapError::PermissionDenied);
                }
            }
            None => {
                return Err(KernelCapError::PermissionDenied);
            }
        }
    }

    let guard = KERNEL_CAP_TABLE.lock();
    let tbl = guard.as_ref().ok_or(KernelCapError::NotSupported)?;
    revocation::revoke(tbl, object_id);
    drop(guard);
    crate::security::exokairos::cap_deadline_table::remove(object_id);
    remove_service_cap_meta(object_id);
    Ok(())
}

#[inline]
pub fn capget(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}

#[inline]
pub fn capset(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}
