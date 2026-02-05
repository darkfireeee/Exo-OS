# exo_crypto

Post-quantum cryptography library for Exo-OS.

## Features

- **ML-KEM (Kyber)**: Key encapsulation mechanism
- **ML-DSA (Dilithium)**: Digital signatures
- **BLAKE3**: Fast cryptographic hashing
- **ChaCha20**: Stream cipher
- **Constant-time operations**: Timing-attack resistant
- **SIMD optimizations**: AVX2 acceleration where available

## Architecture

```
exo_crypto/
├── src/
│   ├── kyber.rs        # ML-KEM key encapsulation
│   ├── dilithium.rs    # ML-DSA signatures
│   ├── chacha20.rs     # Stream cipher
│   ├── hash/           # BLAKE3, SHA3
│   ├── constant_time/  # Timing-safe operations
│   └── simd/           # AVX2 optimizations
└── vendor/pqclean/     # PQClean C sources
```

## Usage

### Key Encapsulation (ML-KEM)

```rust
use exo_crypto::kyber;

let (pk, sk) = kyber::keypair()?;
let (ct, ss) = kyber::encapsulate(&pk)?;
let ss2 = kyber::decapsulate(&ct, &sk)?;
assert_eq!(ss, ss2);
```

### Digital Signatures (ML-DSA)

```rust
use exo_crypto::dilithium;

let (pk, sk) = dilithium::keypair()?;
let sig = dilithium::sign(&message, &sk)?;
dilithium::verify(&sig, &message, &pk)?;
```

### Hashing (BLAKE3)

```rust
use exo_crypto::hash::blake3;

let hash = blake3::hash(b"data");
```

## NIST Standards

- **ML-KEM** (Kyber): FIPS 203 (KEM)
- **ML-DSA** (Dilithium): FIPS 204 (Signatures)

## Performance

- **Kyber-768**: ~0.5ms keygen, ~0.6ms encaps/decaps
- **Dilithium-65**: ~1.5ms sign, ~0.8ms verify
- **BLAKE3**: 3-5 GB/s (SIMD enabled)

## Security Levels

| Algorithm | NIST Level | Classical | Quantum |
|-----------|------------|-----------|---------|
| ML-KEM-768 | 3 | AES-192 | ~143 bits |
| ML-DSA-65 | 3 | SHA-384 | ~128 bits |

## References

- [NIST Post-Quantum Cryptography](https://csrc.nist.gov/projects/post-quantum-cryptography)
- [PQClean](https://github.com/PQClean/PQClean)
