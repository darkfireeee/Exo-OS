//! Host unit & integration test suite importing the **real** Ring 1 server `[lib]` crates:
//! - `exo_ipc_router::{registry, exocordon}`
//! - `exo_init_server::{service, service_table, dependency, supervisor, service_manager}`
//! - `exo_crypto_server::{keystore, wire}`
//! - `exo_vfs_server::{mount_table, compat}`
//! - `exo_shield::ml::trained_weights`

#[cfg(test)]
mod tests_ipc_router {
    use exo_ipc_router::exocordon::{self, IpcError, ServiceId};
    use exo_ipc_router::registry::Registry;
    use std::sync::Mutex;

    static EXOCORDON_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_fnv64a_deterministic_and_distinct() {
        let h1 = Registry::hash_name(b"ipc_router");
        let h2 = Registry::hash_name(b"vfs_server");
        let h3 = Registry::hash_name(b"crypto_server");
        let h4 = Registry::hash_name(b"init_server");
        assert_eq!(h1, Registry::hash_name(b"ipc_router"));
        assert_ne!(h1, h2);
        assert_ne!(h1, h3);
        assert_ne!(h1, h4);
        assert_ne!(h2, h3);
        assert_ne!(h2, h4);
        assert_ne!(h3, h4);
        assert_eq!(Registry::hash_name(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn test_register_resolve_and_idempotent_update() {
        let mut r = Registry::new();
        assert_eq!(r.resolve(b"unknown"), None);
        assert!(r.register(b"vfs_server", 3));
        assert!(r.register(b"crypto_server", 4));
        assert_eq!(r.resolve(b"vfs_server"), Some(3));
        assert_eq!(r.resolve(b"crypto_server"), Some(4));
        assert!(r.register(b"vfs_server", 33));
        assert_eq!(r.resolve(b"vfs_server"), Some(33));
        assert_eq!(r.count(), 2);
    }

    #[test]
    fn test_register_table_full_returns_false() {
        let mut r = Registry::new();
        for i in 0u32..64 {
            let name = format!("svc{i:03}");
            assert!(r.register(name.as_bytes(), i), "slot {i} must be free");
        }
        assert_eq!(r.count(), 64);
        assert!(!r.register(b"svc_overflow", 99));
        assert_eq!(r.count(), 64);
        for i in 0u32..64 {
            let name = format!("svc{i:03}");
            assert_eq!(r.resolve(name.as_bytes()), Some(i));
        }
    }

    #[test]
    fn test_exocordon_blocks_network_to_crypto_direct() {
        let _guard = EXOCORDON_TEST_LOCK.lock().unwrap();
        exocordon::reset_quotas();
        assert_eq!(
            exocordon::check_ipc(ServiceId::Network as u32, ServiceId::Crypto as u32),
            Err(IpcError::UnauthorizedPath)
        );
    }

    #[test]
    fn test_exocordon_allows_vfs_to_crypto_and_isolates_quotas() {
        let _guard = EXOCORDON_TEST_LOCK.lock().unwrap();
        exocordon::reset_quotas();
        assert_eq!(
            exocordon::check_ipc(ServiceId::Vfs as u32, ServiceId::Crypto as u32),
            Ok(())
        );
        for _ in 0..10_000 {
            assert_eq!(
                exocordon::check_ipc(ServiceId::Init as u32, ServiceId::Memory as u32),
                Ok(())
            );
        }
        assert_eq!(
            exocordon::check_ipc(ServiceId::Init as u32, ServiceId::Memory as u32),
            Err(IpcError::QuotaExhausted)
        );
        assert_eq!(
            exocordon::check_ipc(ServiceId::Vfs as u32, ServiceId::Crypto as u32),
            Ok(())
        );
    }
}

#[cfg(test)]
mod tests_init_server {
    use exo_init_server::{dependency, service_table, supervisor, Service, SERVICE_STABLE_MS};

    #[test]
    fn test_service_initial_pid_and_exponential_backoff() {
        let svc = Service::new("test_svc", b"/sbin/test\0");
        assert_eq!(svc.current_pid(), 0);
        assert_eq!(svc.delay(), 1);

        svc.set_pid_at(42, 100);
        assert_eq!(svc.current_pid(), 42);
        svc.mark_dead_at(200);
        assert_eq!(svc.current_pid(), 0);
        assert_eq!(svc.delay(), 2);

        svc.mark_dead_at(300);
        assert_eq!(svc.delay(), 4);
        svc.mark_dead_at(400);
        assert_eq!(svc.delay(), 8);
        svc.mark_dead_at(500);
        assert_eq!(svc.delay(), 16);
        svc.mark_dead_at(600);
        assert_eq!(svc.delay(), 32);

        for _ in 0..20 {
            svc.mark_dead_at(700);
        }
        assert_eq!(svc.delay(), 32);

        // Running beyond SERVICE_STABLE_MS resets backoff back to 1
        svc.set_pid_at(43, 1_000);
        svc.mark_dead_at(1_000 + SERVICE_STABLE_MS);
        assert_eq!(svc.delay(), 1);
    }

    #[test]
    fn test_canonical_service_table_and_dependencies() {
        assert_eq!(service_table::CANONICAL_SERVICES.len(), service_table::SERVICE_COUNT);
        assert_eq!(service_table::CANONICAL_SERVICES[0].name, "ipc_router");
        assert_eq!(
            dependency::metadata("memory_server").unwrap().requires,
            &["ipc_router"]
        );
        assert_eq!(
            dependency::metadata("vfs_server").unwrap().requires,
            &["ipc_router", "memory_server"]
        );
        assert_eq!(
            dependency::metadata("exo_compositor").unwrap().requires,
            &["ipc_router", "fb_server", "vfs_server"]
        );
        assert!(dependency::metadata("exosh")
            .unwrap()
            .requires
            .contains(&"exo_shield"));
        assert!(supervisor::is_strate1_service("fb_server"));
        assert!(!supervisor::is_strate1_service("exo_compositor"));
    }

    #[test]
    fn test_reap_marks_only_dead_pid() {
        let services = [
            Service::new("ipc_router", service_table::IPC_ROUTER_BIN),
            Service::new("vfs_server", service_table::VFS_SERVER_BIN),
        ];
        services[0].set_pid_at(2, 10);
        services[1].set_pid_at(3, 10);
        let idx = supervisor::note_child_exit(&services, 3);
        assert_eq!(idx, Some(1));
        assert_eq!(services[0].current_pid(), 2);
        assert_eq!(services[1].current_pid(), 0);
        assert!(services[1].is_dead());
    }
}

#[cfg(test)]
mod tests_crypto_server {
    use exo_crypto_server::keystore::{self, KeyType, KEY_SIZE};
    use exo_crypto_server::wire;
    use std::sync::Mutex;

    static KEYSTORE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_hkdf_blake3_deterministic_and_non_zero() {
        let mut out1 = [0u8; KEY_SIZE];
        let mut out2 = [0u8; KEY_SIZE];
        let mut out3 = [0u8; KEY_SIZE];
        wire::derive_key_hkdf(b"master_key_material", &mut out1);
        wire::derive_key_hkdf(b"master_key_material", &mut out2);
        wire::derive_key_hkdf(b"other_key_material", &mut out3);
        assert_eq!(out1, out2);
        assert_ne!(out1, [0u8; KEY_SIZE]);
        assert_ne!(out1, out3);
    }

    #[test]
    fn test_keystore_opaque_handles_owner_isolation_rotation_and_revocation() {
        let _guard = KEYSTORE_TEST_LOCK.lock().unwrap();
        keystore::keystore_init();

        let mut derived = [0u8; KEY_SIZE];
        wire::derive_key_hkdf(b"srv04-owner-42", &mut derived);
        let handle = keystore::insert_key(&derived, KeyType::Derived, 42);
        assert_ne!(handle, 0);

        // Correct owner can fetch key; foreign owner is rejected (CAP-01)
        let (fetched, kt) = keystore::get_key(handle, 42).expect("owner fetch");
        assert_eq!(fetched, derived);
        assert_eq!(kt, KeyType::Derived);
        assert!(keystore::get_key(handle, 99).is_none());

        // Rotation replaces key material in place under same handle
        assert_eq!(keystore::rotate_key(handle, 42), handle);
        let (rotated, _) = keystore::get_key(handle, 42).expect("rotated fetch");
        assert_ne!(rotated, derived);

        // Revocation shreds key and invalidates handle
        assert!(keystore::revoke_key(handle));
        assert!(keystore::get_key(handle, 42).is_none());
        assert!(!keystore::revoke_key(handle));
    }

    #[test]
    fn test_keystore_per_owner_quota_enforced() {
        let _guard = KEYSTORE_TEST_LOCK.lock().unwrap();
        keystore::keystore_init();

        let key = [0x5Au8; KEY_SIZE];
        let mut handles = Vec::new();
        for _ in 0..8 {
            let h = keystore::insert_key(&key, KeyType::Encryption, 777);
            assert_ne!(h, 0);
            handles.push(h);
        }
        assert_eq!(keystore::insert_key(&key, KeyType::Encryption, 777), 0);
        for h in handles {
            assert!(keystore::revoke_key(h));
        }
    }
}

#[cfg(test)]
mod tests_vfs_server {
    use exo_vfs_server::mount_table::{
        check_vfs_write_access, fnv32, parse_and_mount, FsType, MountTable,
    };

    #[test]
    fn test_fnv32_distinct_pseudo_fs() {
        assert_eq!(fnv32(b""), 2166136261u32);
        assert_ne!(fnv32(b"/proc"), fnv32(b"/sys"));
        assert_ne!(fnv32(b"/proc"), fnv32(b"/dev"));
        assert_ne!(fnv32(b"/sys"), fnv32(b"/dev"));
    }

    #[test]
    fn test_mount_table_defaults_replace_remove_and_capacity() {
        let mut t = MountTable::new();
        t.mount_defaults();
        assert_eq!(t.active_count(), 3);
        assert_eq!(t.find_by_path(b"/proc").unwrap().fs_type, FsType::ProcFs);
        assert_eq!(t.find_by_path(b"/sys").unwrap().fs_type, FsType::SysFs);
        assert_eq!(t.find_by_path(b"/dev").unwrap().fs_type, FsType::DevFs);

        assert!(t.remove(b"/proc").is_some());
        assert_eq!(t.active_count(), 2);
        assert!(t.find_by_path(b"/proc").is_none());

        let mut full = MountTable::new();
        for i in 0u64..32 {
            let p = format!("/mnt/v{i}");
            full.add_or_replace(FsType::ExoFs, p.as_bytes(), i).unwrap();
        }
        assert_eq!(
            full.add_or_replace(FsType::ExoFs, b"/mnt/overflow", 0xFFFF),
            Err(exo_syscall_abi::ENOSPC)
        );
    }

    #[test]
    fn test_parse_and_mount_wire_payload_and_write_guard() {
        let mut t = MountTable::new();
        assert_eq!(parse_and_mount(&mut t, &[0u8; 5]), Err(exo_syscall_abi::EINVAL));

        let mut payload = [0u8; 32];
        payload[0] = 1; // ExoFs
        let blob: u64 = 0x1122_3344_5566_7788;
        payload[5..13].copy_from_slice(&blob.to_le_bytes());
        payload[13] = b'/';
        let (_, out_blob) = parse_and_mount(&mut t, &payload).expect("valid mount");
        assert_eq!(out_blob, blob);
        assert_eq!(t.find_by_path(b"/").unwrap().root_blob, blob);

        assert!(check_vfs_write_access(1));
        // FIX-10: static PID 5 and Ring 3 PID >= 10 must NOT bypass VFS write access.
        assert!(!check_vfs_write_access(5));
        assert!(!check_vfs_write_access(10));
        assert!(!check_vfs_write_access(25));
        assert!(!check_vfs_write_access(3));
        assert!(!check_vfs_write_access(7));
        assert!(exo_vfs_server::mount_table::check_vfs_write_access_with_class(
            14,
            exo_syscall_abi::SVC_VFS_SERVER
        ));
        assert!(!exo_vfs_server::mount_table::check_vfs_write_access_with_class(
            25,
            exo_syscall_abi::SVC_UNKNOWN
        ));
        assert!(exo_vfs_server::mount_table::is_safe_vfs_path(b"/tmp/safe.txt"));
        assert!(!exo_vfs_server::mount_table::is_safe_vfs_path(b"/tmp/../etc/shadow"));
        assert!(!exo_vfs_server::mount_table::is_safe_vfs_path(b"relative/path"));
    }
}

#[cfg(test)]
mod tests_network_server_security {
    use exo_network_server::socket_table::{service_allows_raw_socket, SocketKind};

    #[test]
    fn var12_network_server_rejects_unprivileged_sock_raw_for_ring3_pids() {
        const AF_INET: u32 = 2;
        const SOCK_RAW: u32 = 3;
        const IPPROTO_ICMP: u32 = 1;

        // Unprivileged Ring 3 PID >= 18 MUST be denied SOCK_RAW (fixes `|| sender_pid >= 18`).
        assert_eq!(
            SocketKind::from_domain_type_privileged(AF_INET, SOCK_RAW, IPPROTO_ICMP, 25),
            Err(exo_syscall_abi::EPERM)
        );
        assert_eq!(
            SocketKind::from_domain_type_privileged(AF_INET, SOCK_RAW, 0, 18),
            Err(exo_syscall_abi::EPERM)
        );
        assert!(!service_allows_raw_socket(25, exo_syscall_abi::SVC_UNKNOWN, IPPROTO_ICMP));

        // Kernel-attested init_server / network_server may open SOCK_RAW.
        assert_eq!(
            SocketKind::from_domain_type_for_service(
                AF_INET,
                SOCK_RAW,
                IPPROTO_ICMP,
                7,
                exo_syscall_abi::SVC_NETWORK_SERVER
            ),
            Ok(SocketKind::Raw)
        );
        // Compositor/exosh may only use ICMP (protocol=1), NOT arbitrary raw protocol=0.
        assert!(service_allows_raw_socket(16, exo_syscall_abi::SVC_EXO_COMPOSITOR, IPPROTO_ICMP));
        assert!(!service_allows_raw_socket(16, exo_syscall_abi::SVC_EXO_COMPOSITOR, 0));
    }
}

#[cfg(test)]
mod tests_exo_shield_weights {
    use exo_shield::ml::trained_weights;

    #[test]
    fn test_binary_weights_asset_decoded_faithfully() {
        assert_eq!(trained_weights::TRAINED_MLP_VERSION, 2);
        assert_eq!(trained_weights::TRAINED_MLP_CHECKSUM, 0x7EB2_EAD4_B180_D76D);
        assert_eq!(trained_weights::FEATURE_MAX[0], 100);
        assert_eq!(trained_weights::MLP_W1[0], -16311);
        assert_eq!(trained_weights::IF_NODE_THRESHOLD[503], 1);
    }
}

#[cfg(test)]
mod tests_security_p0_p1 {
    use exo_ipc_router::router;
    use exo_shield::{engine, ipc_gate, sandbox};
    use std::sync::Mutex;

    static SHIELD_SEC_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn var01_ipc_router_stamps_authenticated_sender_pid() {
        let mut forged = [0u8; 32];
        // Attacker tries to forge sender_pid = 0 (kernel) in inner ShieldRequest
        forged[0..4].copy_from_slice(&0u32.to_le_bytes());
        forged[4..8].copy_from_slice(&2u32.to_le_bytes()); // QUARANTINE_CMD
        let mut out = [0u8; 192];

        assert!(router::stamp_routed_payload(0, &forged, &mut out).is_none());
        let len = router::stamp_routed_payload(25, &forged, &mut out).expect("stamped");
        assert_eq!(len, forged.len());
        assert_eq!(u32::from_le_bytes([out[0], out[1], out[2], out[3]]), 25);
    }

    #[test]
    fn var07_exo_shield_cap_precedence_and_no_pid_le2_bypass() {
        let _guard = SHIELD_SEC_LOCK.lock().unwrap();
        ipc_gate::policy_init();

        const QUARANTINE_CMD: u32 = 2;
        const POLICY_UPDATE: u32 = 4;
        const SHIELD_ERR_CAP: u32 = 5;

        // (a) Caller PID 25 WITH a valid ExoCapTokenWire at payload[172..192]
        // MUST succeed (NOT blocked prematurely by WILDCARD_PID -> Deny).
        let mut payload = [0u8; 192];
        payload[172..192].fill(0xA5);
        let ok_q = ipc_gate::authorize_ipc_request(25, QUARANTINE_CMD, &payload, |_| true);
        assert_eq!(ok_q.err_status, None);
        let ok_p = ipc_gate::authorize_ipc_request(25, POLICY_UPDATE, &payload, |_| true);
        assert_eq!(ok_p.err_status, None);

        // (b) Caller PID 2 (and PID 1) WITHOUT a valid ExoCapTokenWire
        // MUST be rejected with SHIELD_ERR_CAP (5).
        let empty_payload = [0u8; 192];
        let deny_pid2 =
            ipc_gate::authorize_ipc_request(2, QUARANTINE_CMD, &empty_payload, |_| false);
        assert_eq!(deny_pid2.err_status, Some(SHIELD_ERR_CAP));
        let deny_pid1 =
            ipc_gate::authorize_ipc_request(1, QUARANTINE_CMD, &empty_payload, |_| false);
        assert_eq!(deny_pid1.err_status, Some(SHIELD_ERR_CAP));
    }

    #[test]
    fn var09_ipc_violation_burst_escalates_to_quarantine_then_kill() {
        let _guard = SHIELD_SEC_LOCK.lock().unwrap();
        engine::realtime_init();
        let pid = 409u32;
        engine::reset_violation_escalation(pid);

        let (_, a1) = engine::record_violation_escalation(pid, engine::ThreatLevel::High);
        let (_, a2) = engine::record_violation_escalation(pid, engine::ThreatLevel::High);
        let (_, a3) = engine::record_violation_escalation(pid, engine::ThreatLevel::High);
        let (_, a4) = engine::record_violation_escalation(pid, engine::ThreatLevel::High);
        let (_, a5) = engine::record_violation_escalation(pid, engine::ThreatLevel::High);

        assert_eq!(a1 as u8, engine::ContainmentAction::Throttle as u8);
        assert_eq!(a2 as u8, engine::ContainmentAction::Throttle as u8);
        assert_eq!(a3 as u8, engine::ContainmentAction::Quarantine as u8);
        assert_eq!(a4 as u8, engine::ContainmentAction::Quarantine as u8);
        assert_eq!(a5 as u8, engine::ContainmentAction::Kill as u8);

        // Also confirm default realtime EventFilter matches EventType::Ipc at High severity.
        let ev = engine::MonitoredEvent {
            pid,
            event_type: engine::EventType::Ipc,
            opcode: 0,
            arg0: 4,
            arg1: 0,
            timestamp: 100,
            severity: engine::ThreatLevel::High,
        };
        let res = engine::submit_event(&ev, 100);
        assert!(res.action as u8 >= engine::FilterAction::Alert as u8);
        engine::reset_violation_escalation(pid);
    }

    #[test]
    fn var10_quarantine_container_manager_handles_beyond_8_and_overflow() {
        let _guard = SHIELD_SEC_LOCK.lock().unwrap();
        sandbox::container::container_manager_init();

        // Fill well past the old MAX_CONTAINERS=8 and even beyond new MAX_CONTAINERS=64.
        for pid in 101u32..=170u32 {
            assert!(
                sandbox::quarantine_pid(pid),
                "quarantine_pid({pid}) must never silently fail on container overflow"
            );
            assert!(sandbox::is_pid_quarantined(pid));
        }
        // Latest PID (170) is actively quarantined and denied arbitrary syscalls.
        assert!(!sandbox::quarantine_allows_syscall(170, 59));
    }
}
