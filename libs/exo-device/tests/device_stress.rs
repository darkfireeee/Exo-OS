#[test]
fn device_ports_stress() {
    assert_eq!(exo_device::DEVICE_PORTS.len(), 1);
    assert_ne!(exo_device::device_stress_signature(100_000), 0);
}
