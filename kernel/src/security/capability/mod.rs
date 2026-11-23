// kernel/src/security/capability/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAPABILITY — Root module (Exo-OS Security · Layer 2b) — v6
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE CAP-01: security/capability/ is the UNIQUE source of truth for capabilities.
// RULE CAP-02 (v6): ipc/, fs/, process/ access via security::access_control::check_access()
//                   — direct call to verify() outside of security/ is FORBIDDEN.
// RULE CAP-03: Delegation = always with a subset of rights (invariant CAP-03).
//
// Submodules:
//   token      — CapToken (24 bytes, unforgeable, Copy)
//   rights     — Rights bitmask (u32)
//   table      — CapTable per process (512 slots, lock-free reads)
//   verify     — verify() UNIQUE entry point + CapError (v6)
//   revocation — revoke() O(1) only (v6)
//   delegation — subdelegation (invariant CAP-03)
//   namespace  — independent ObjectId domains
// ═══════════════════════════════════════════════════════════════════════════════

pub mod delegation;
pub mod namespace;
pub mod revocation;
pub mod rights;
pub mod table;
pub mod token;
pub mod verify;

// ── Public re-exports ────────────────────────────────────────────────────────

// Fundamental types
pub use rights::Rights;
pub use table::{CapTable, CapTableSnapshot, CAP_TABLE_CAPACITY};
pub use token::{
    read_stats as token_stats, CapObjectType, CapToken, ObjectId, TokenStats, CAP_TOKEN_WIRE_SIZE,
};

// verify — v6: extracted from revocation.rs
pub use verify::{
    verify, verify_and_get_rights, verify_ipc_recv, verify_ipc_send, verify_read,
    verify_read_write, verify_typed, CapError,
};

// revocation — v6: revocation only
pub use revocation::{revoke, revoke_token};

pub use delegation::{
    can_delegate, delegate, delegate_all, delegate_read_only, DelegationChain, DelegationEntry,
};
pub use namespace::{alloc_namespace_id, cross_namespace_verify, CapNamespace, NamespaceId};

// ─────────────────────────────────────────────────────────────────────────────
// init_capability_subsystem — called at boot (sequenced step)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static CAP_INIT_DONE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Global capability table (kernel-context)
// ─────────────────────────────────────────────────────────────────────────────

use crate::scheduler::sync::spinlock::SpinLock;

/// Kernel capability table — used for syscall allocations
/// before the process registry is available.
/// Initialized by `init_capability_subsystem()`.
static KERNEL_CAP_TABLE: SpinLock<Option<table::CapTable>> = SpinLock::new(None);

/// Ring 1 service capability metadata.
///
/// Tokens created via `exo_cap_create()` for IPC endpoints are
/// registered here to allow the kernel to verify the pair
/// `{calling service -> target service}` when a server validates a token
/// received on the wire.
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

/// Unique, monotonically increasing ObjectId generator.
static OBJ_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocates a fresh ObjectId, guaranteed unique for the kernel lifetime.
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

/// Initializes the capability subsystem.
///
/// # Boot sequence
/// Called by `security::init()` before any other layer 2b+ initialization.
/// Must be called ONCE; subsequent calls are idempotent.
pub fn init_capability_subsystem() {
    if CAP_INIT_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    // Initialize global kernel table
    *KERNEL_CAP_TABLE.lock() = Some(table::CapTable::new());
}

/// Returns true if the subsystem is initialized.
#[inline(always)]
pub fn is_initialized() -> bool {
    CAP_INIT_DONE.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelCapError — syscall error (to_kernel_errno compatible)
// ─────────────────────────────────────────────────────────────────────────────

/// Error returned by capability syscall wrappers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCapError {
    NotFound,
    PermissionDenied,
    InvalidArg,
    NotSupported,
}

impl KernelCapError {
    /// Converts to negative errno code (Linux ABI compatible).
    #[inline]
    pub const fn to_kernel_errno(self) -> i32 {
        match self {
            Self::NotFound => -2,         // ENOENT
            Self::PermissionDenied => -1, // EPERM
            Self::InvalidArg => -22,      // EINVAL
            Self::NotSupported => -38,    // ENOSYS
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Syscall wrappers — called from syscall/table.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Creates an IPC service capability for a target endpoint.
///
/// `cap_type`: object type (only `IpcEndpoint` is accepted on this path),
/// `rights`: Rights bitmask, `target_pid`: target service PID,
/// `owner_pid`: PID of the service that will receive the token.
///
/// Returns the `CapToken` issued by the kernel or an error.
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
    // Rights bitmask validation
    let rights_val = Rights::from_bits(rights).ok_or(KernelCapError::InvalidArg)?;
    // Object type validation and conversion
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
    // T20-001: Ring 3 callers can NEVER mint `IPC_MANAGE` (reserved for Ring 0
    // kernel-authenticated control messages such as `PHOENIX_WAKE_ENTROPY`).
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

    // Allocate a fresh ObjectId and insert into kernel table
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

    // FIX-P1-KAIROS (Security_Application_Audit §GAP-07): associate a temporal
    // deadline with the token via ExoKairos (TTL function of rights). Best-effort —
    // without regression if clock is uncalibrated or table is full.
    crate::security::exokairos::register_ttl_for_cap(oid, rights_val);

    Ok(token)
}

/// Mints a Ring 0 kernel-authenticated IPC capability token (e.g. `PHOENIX_WAKE_ENTROPY`
/// with `Rights::IPC_SEND | Rights::IPC_MANAGE`, `T20-001`).
///
/// Userspace `exo_cap_create()` can never mint `Rights::IPC_MANAGE`, so servers verifying
/// `IPC_SEND | IPC_MANAGE` via `exo_cap_check` get cryptographic proof that the message
/// originated from Ring 0. Caller MUST revoke the token once consumed.
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

/// Mints a `CAP_PHOENIX_FAULT_INJECT` capability reserved exclusively for the qualification
/// runner under `#[cfg(feature = "phoenix-fault-injection")]` (P5.0).
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

/// Verifies a `CAP_PHOENIX_FAULT_INJECT` token presented by `caller_pid` (P5.0).
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
///
/// Used by Ring 1 servers that wish to validate that an IPC request
/// carries a kernel capability corresponding to the expected service.
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

/// Revokes a capability by opaque handle (syscall exo_cap_revoke).
///
/// T11-001 FIX: Verifies that the calling process is indeed the owner
/// or registered recipient of the capability.
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
                // Without metadata proving ownership, a non-root caller cannot revoke.
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

/// Reading POSIX.1e capabilities (syscall capget — Linux compat).
///
/// ExoOS uses its own capability model (CapToken/Rights/CapTable),
/// incompatible with POSIX.1e capget/capset. Returning ENOSYS is
/// the correct and documented behavior — ExoOS applications
/// must use exo_cap_create/exo_cap_revoke syscalls instead.
#[inline]
pub fn capget(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}

/// Writing POSIX.1e capabilities (syscall capset — Linux compat).
#[inline]
pub fn capset(_hdrp: u64, _datap: u64) -> Result<(), KernelCapError> {
    Err(KernelCapError::NotSupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::core::pid::Pid;
    use crate::security::ipc_policy::{register_service_class, unregister_service, ServiceClass};

    static INIT: std::sync::Once = std::sync::Once::new();

    fn ensure_capability_init() {
        INIT.call_once(init_capability_subsystem);
    }

    fn register_test_service(pid: u32, class: ServiceClass) {
        let pid = Pid(pid);
        let _ = unregister_service(pid);
        assert!(register_service_class(pid, class));
    }

    fn unregister_test_service(pid: u32) {
        let _ = unregister_service(Pid(pid));
    }

    #[test]
    fn service_token_roundtrip_for_allowed_route() {
        ensure_capability_init();
        const CRYPTO_PID: u32 = 1101;
        const EXO_SHIELD_PID: u32 = 1102;

        register_test_service(CRYPTO_PID, ServiceClass::CryptoServer);
        register_test_service(EXO_SHIELD_PID, ServiceClass::ExoShield);

        let token = create(
            CapObjectType::IpcEndpoint as u32,
            Rights::IPC_SEND.bits(),
            CRYPTO_PID,
            EXO_SHIELD_PID,
        )
        .expect("exo_shield -> crypto_server token");

        let verified = check_token(
            token,
            Rights::IPC_SEND.bits(),
            CRYPTO_PID,
            CapObjectType::IpcEndpoint as u32,
        )
        .expect("token accepted");

        assert_eq!(verified, token.object_id());
        let owner_verified = check_token_owner(
            token,
            Rights::IPC_SEND.bits(),
            EXO_SHIELD_PID,
            CRYPTO_PID,
            CapObjectType::IpcEndpoint as u32,
        )
        .expect("owner token accepted");
        assert_eq!(owner_verified, token.object_id());
        assert_eq!(
            check_token_owner(
                token,
                Rights::IPC_SEND.bits(),
                9999,
                CRYPTO_PID,
                CapObjectType::IpcEndpoint as u32,
            ),
            Err(KernelCapError::PermissionDenied)
        );
        let _ = revoke_handle(token.object_id().as_u64() as u32);
        unregister_test_service(CRYPTO_PID);
        unregister_test_service(EXO_SHIELD_PID);
    }

    #[test]
    fn service_token_creation_respects_ipc_policy() {
        ensure_capability_init();
        const CRYPTO_PID: u32 = 1111;
        const NETWORK_PID: u32 = 1112;

        register_test_service(CRYPTO_PID, ServiceClass::CryptoServer);
        register_test_service(NETWORK_PID, ServiceClass::NetworkServer);

        let result = create(
            CapObjectType::IpcEndpoint as u32,
            Rights::IPC_SEND.bits(),
            NETWORK_PID,
            CRYPTO_PID,
        );

        assert_eq!(result, Err(KernelCapError::PermissionDenied));
        unregister_test_service(CRYPTO_PID);
        unregister_test_service(NETWORK_PID);
    }

    /// TIER 0.6 — the **revocation** of an IPC service token is effective: after
    /// `revoke_handle`, the same token is rejected by `check_token` (generation bumped
    /// in KERNEL_CAP_TABLE + meta removed). Completes the e2e capability scenario on
    /// IPC side (denied before issuance → allowed with token → denied after revocation).
    #[test]
    fn service_token_revocation_is_effective() {
        ensure_capability_init();
        const OWNER_PID: u32 = 1131; // ExoShield (authorized route to CryptoServer)
        const TARGET_PID: u32 = 1132; // CryptoServer

        register_test_service(OWNER_PID, ServiceClass::ExoShield);
        register_test_service(TARGET_PID, ServiceClass::CryptoServer);

        let token = create(
            CapObjectType::IpcEndpoint as u32,
            Rights::IPC_SEND.bits(),
            TARGET_PID,
            OWNER_PID,
        )
        .expect("mint service token");

        // Before revocation: token is accepted.
        assert!(
            check_token(
                token,
                Rights::IPC_SEND.bits(),
                TARGET_PID,
                CapObjectType::IpcEndpoint as u32,
            )
            .is_ok(),
            "un token fraîchement émis doit être accepté"
        );

        // Revocation by handle (lower 32 bits of ObjectId).
        revoke_handle(token.object_id().as_u64() as u32).expect("revoke");

        // After revocation: the SAME token is rejected (no bypass).
        assert!(
            check_token(
                token,
                Rights::IPC_SEND.bits(),
                TARGET_PID,
                CapObjectType::IpcEndpoint as u32,
            )
            .is_err(),
            "après révocation, le token doit être rejeté"
        );

        unregister_test_service(OWNER_PID);
        unregister_test_service(TARGET_PID);
    }
}
