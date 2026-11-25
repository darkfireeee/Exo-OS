//! Unit tests for `security::capability` service tokens.

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

#[test]
fn service_token_revocation_is_effective() {
    ensure_capability_init();
    const OWNER_PID: u32 = 1131;
    const TARGET_PID: u32 = 1132;

    register_test_service(OWNER_PID, ServiceClass::ExoShield);
    register_test_service(TARGET_PID, ServiceClass::CryptoServer);

    let token = create(
        CapObjectType::IpcEndpoint as u32,
        Rights::IPC_SEND.bits(),
        TARGET_PID,
        OWNER_PID,
    )
    .expect("mint service token");

    assert!(check_token(
        token,
        Rights::IPC_SEND.bits(),
        TARGET_PID,
        CapObjectType::IpcEndpoint as u32,
    )
    .is_ok());

    revoke_handle(token.object_id().as_u64() as u32).expect("revoke");

    assert!(check_token(
        token,
        Rights::IPC_SEND.bits(),
        TARGET_PID,
        CapObjectType::IpcEndpoint as u32,
    )
    .is_err());

    unregister_test_service(OWNER_PID);
    unregister_test_service(TARGET_PID);
}
