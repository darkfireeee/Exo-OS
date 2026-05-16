#[test]
fn crypto_ports_stress() {
    assert_eq!(exo_crypto::CRYPTO_PORTS.len(), 10);
    assert_ne!(exo_crypto::crypto_stress_signature(100_000), 0);
}
