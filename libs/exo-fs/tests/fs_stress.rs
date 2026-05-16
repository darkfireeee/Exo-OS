#[test]
fn fs_ports_stress() {
    assert_eq!(exo_fs::FS_PORTS.len(), 3);
    assert_ne!(exo_fs::fs_stress_signature(100_000), 0);
}
