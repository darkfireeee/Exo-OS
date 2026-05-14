#[test]
fn graphics_ports_stress() {
    assert_eq!(exo_graphics::GRAPHICS_PORTS.len(), 3);
    assert_ne!(exo_graphics::graphics_stress_signature(100_000), 0);
}
