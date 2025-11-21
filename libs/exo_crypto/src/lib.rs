// libs/exo_crypto/src/lib.rs
#![no_std]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

pub mod kyber;
pub mod dilithium;
pub mod chacha20;

// Réexportations
pub use kyber::{kyber_keypair, kyber_encaps, kyber_decaps, KYBER_PUBLICKEYBYTES, KYBER_SECRETKEYBYTES, KYBER_CIPHERTEXTBYTES, KYBER_BYTES};
pub use dilithium::{dilithium_keypair, dilithium_sign, dilithium_verify, DILITHIUM_PUBLICKEYBYTES, DILITHIUM_SECRETKEYBYTES, DILITHIUM_BYTES};
pub use chacha20::{ChaCha20, XChaCha20, POLY1305_KEYBYTES, POLY1305_TAGBYTES};

use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialise la bibliothèque cryptographique
pub fn init() {
    if INITIALIZED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        // Initialiser les générateurs aléatoires
        init_random();
        log::info!("exo_crypto initialized");
    }
}

/// Initialise le générateur aléatoire
fn init_random() {
    // Dans un vrai OS, cela utiliserait le TRNG matériel
    // et le mélangerait avec des entropies système
    #[cfg(test)]
    {
        // Pour les tests, utiliser un CSPRNG simple
        use rand::{rngs::StdRng, SeedableRng};
        let _ = StdRng::from_entropy();
    }
}

/// Fonction de hash rapide pour les opérations internes
/// ATTENTION: Pas pour une utilisation cryptographique directe
pub fn fast_hash(data: &[u8]) -> u64 {
    // Djb2 hash modifié pour être plus rapide
    let mut hash: u64 = 5381;
    
    for byte in data {
        hash = ((hash << 5) + hash) ^ (*byte as u64);
    }
    
    hash
}