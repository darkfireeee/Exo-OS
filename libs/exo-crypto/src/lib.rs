#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoBoundary {
    KernelNoStdPrimitive,
    CryptoServerPrimitive,
    RejectedFfiService,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoVerdict {
    Native,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CryptoPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub boundary: CryptoBoundary,
    pub verdict: CryptoVerdict,
    pub phoenix_policy: &'static str,
}

pub const CRYPTO_PORTS: &[CryptoPort] = &[
    CryptoPort {
        name: "ring",
        vendor_tree: "ring-upstream",
        boundary: CryptoBoundary::CryptoServerPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "crypto_server-recreates-runtime-state",
    },
    CryptoPort {
        name: "RustCrypto AEADs",
        vendor_tree: "rustcrypto-aeads-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
    CryptoPort {
        name: "RustCrypto block ciphers",
        vendor_tree: "rustcrypto-block-ciphers-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
    CryptoPort {
        name: "RustCrypto stream ciphers",
        vendor_tree: "rustcrypto-stream-ciphers-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
    CryptoPort {
        name: "RustCrypto hashes",
        vendor_tree: "rustcrypto-hashes-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
    CryptoPort {
        name: "RustCrypto KDFs",
        vendor_tree: "rustcrypto-kdfs-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
    CryptoPort {
        name: "RustCrypto password hashes",
        vendor_tree: "rustcrypto-password-hashes-upstream",
        boundary: CryptoBoundary::CryptoServerPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "no-secret-cache-survives-switch",
    },
    CryptoPort {
        name: "RustCrypto RSA",
        vendor_tree: "rustcrypto-rsa-upstream",
        boundary: CryptoBoundary::CryptoServerPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "private-keys-stay-in-crypto-server",
    },
    CryptoPort {
        name: "RustCrypto elliptic curves",
        vendor_tree: "rustcrypto-elliptic-curves-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
    CryptoPort {
        name: "RustCrypto traits",
        vendor_tree: "rustcrypto-traits-upstream",
        boundary: CryptoBoundary::KernelNoStdPrimitive,
        verdict: CryptoVerdict::Native,
        phoenix_policy: "stateless",
    },
];

pub fn crypto_port_allowed(name: &str) -> bool {
    CRYPTO_PORTS
        .iter()
        .find(|port| port.name == name)
        .map(|port| port.verdict != CryptoVerdict::Rejected)
        .unwrap_or(false)
}

pub fn crypto_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f43_5259_u64;
    for i in 0..iterations.max(1) {
        let port = CRYPTO_PORTS[i as usize % CRYPTO_PORTS.len()];
        acc = acc.rotate_left(11) ^ port.vendor_tree.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libsodium_is_removed_not_adapted() {
        assert!(!crypto_port_allowed("libsodium"));
        assert!(CRYPTO_PORTS.iter().all(|port| port.name != "libsodium"));
    }
}
