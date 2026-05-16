#[test]
fn posix_ports_stress() {
    assert_eq!(exo_posix::POSIX_PORTS.len(), 1);
    assert_ne!(exo_posix::posix_stress_signature(100_000), 0);
}
