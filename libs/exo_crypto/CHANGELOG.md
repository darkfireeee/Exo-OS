# Changelog

All notable changes to exo_crypto will be documented in this file.

## [Unreleased]

### Added
- ML-KEM-768 (Kyber) key encapsulation
- ML-DSA-65 (Dilithium) digital signatures
- ChaCha20 stream cipher
- BLAKE3 hashing (planned)
- SHA3 support (planned)
- Constant-time comparison operations
- SIMD optimizations (AVX2) when available
- PQClean C sources integration

### Performance
- Kyber-768: ~0.5ms keygen
- Dilithium-65: ~1.5ms sign
- BLAKE3: 3-5 GB/s target

### Security
- Timing-attack resistant implementations
- NIST PQC standards compliant
- Constant-time primitives

## [0.1.0] - 2026-02-05

### Added
- Initial structure with Kyber, Dilithium, ChaCha20 stubs
- PQClean vendor sources (ML-KEM-768, ML-DSA-65)
- Module organization
