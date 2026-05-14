use std::path::PathBuf;

#[test]
fn network_vendors_are_present() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vendors");
    for port in exo_network::NETWORK_PORTS {
        assert!(root.join(port.vendor_tree).join(".git").is_dir());
    }
}

#[test]
fn smoltcp_adapter_stress() {
    let signature = exo_network::smoltcp_stress_signature(100_000);
    assert_ne!(signature, 0);
    assert_eq!(exo_network::clamp_mtu(64), 576);
    assert_eq!(exo_network::clamp_mtu(9000), 1500);
    assert_eq!(exo_network::socket_budget(0), 1);
    assert_eq!(exo_network::socket_budget(4096), 64);
}
