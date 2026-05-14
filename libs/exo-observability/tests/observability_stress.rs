#[test]
fn log_adapter_stress() {
    assert_ne!(
        exo_observability::observability_stress_signature(100_000),
        0
    );
    assert_eq!(exo_observability::severity_score(log::Level::Error), 5);
    assert_eq!(exo_observability::OBSERVABILITY_PORTS.len(), 2);
}
