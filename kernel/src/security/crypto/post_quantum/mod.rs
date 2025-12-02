//! Post-Quantum Cryptography Module
//!
//! NIST-standardized post-quantum algorithms:
//! - Kyber: Key Encapsulation Mechanism (KEM)
//! - Dilithium: Digital Signature Algorithm
//!
//! These are production-ready implementations resistant to quantum attacks.

pub mod dilithium;
pub mod kyber;

pub use dilithium::{Dilithium2, Dilithium3, Dilithium5, DilithiumSignature};
pub use kyber::{Kyber1024, Kyber512, Kyber768, KyberKem};

/// Initialize post-quantum crypto
pub fn init() {
    log::info!("Post-quantum crypto initialized");
    log::info!("  - Kyber KEM (NIST standard)");
    log::info!("  - Dilithium signatures (NIST standard)");
}

/// Security levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityLevel {
    /// 128-bit security (equivalent to AES-128)
    Level1 = 1,
    /// 192-bit security (equivalent to AES-192)  
    Level3 = 3,
    /// 256-bit security (equivalent to AES-256)
    Level5 = 5,
}
