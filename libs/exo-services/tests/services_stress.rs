#[test]
fn service_ports_stress() {
    assert_eq!(exo_services::SERVICE_PORTS.len(), 1);
    assert_ne!(exo_services::services_stress_signature(100_000), 0);
}
