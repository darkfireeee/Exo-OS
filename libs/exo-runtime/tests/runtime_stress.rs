#[test]
fn runtime_ports_stress() {
    assert_eq!(exo_runtime::RUNTIME_PORTS.len(), 4);
    assert_ne!(exo_runtime::runtime_stress_signature(100_000), 0);
}
