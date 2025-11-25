// libs/exo_crypto/src/dilithium.rs
#![allow(non_snake_case)]

/// Taille de la clé publique Dilithium
pub const DILITHIUM_PUBLICKEYBYTES: usize = 1312;
/// Taille de la clé secrète Dilithium
pub const DILITHIUM_SECRETKEYBYTES: usize = 2528;
/// Taille de la signature Dilithium
pub const DILITHIUM_BYTES: usize = 2420;

/// Génère une paire de clés Dilithium
///
/// # Arguments
/// * `pk` - Tableau pour stocker la clé publique
/// * `sk` - Tableau pour stocker la clé secrète
///
/// # Retour
/// `true` si la génération a réussi, `false` sinon
pub fn dilithium_keypair(pk: &mut [u8; DILITHIUM_PUBLICKEYBYTES], sk: &mut [u8; DILITHIUM_SECRETKEYBYTES]) -> bool {
    #[cfg(not(test))]
    {
        extern "C" {
            fn crypto_sign_keypair(pk: *mut u8, sk: *mut u8) -> i32;
        }
        
        let result = unsafe { crypto_sign_keypair(pk.as_mut_ptr(), sk.as_mut_ptr()) };
        result == 0
    }
    
    #[cfg(test)]
    {
        // Génération déterministe pour les tests
        pk.iter_mut().enumerate().for_each(|(i, b)| *b = (i % 256) as u8);
        sk.iter_mut().enumerate().for_each(|(i, b)| *b = (i % 256) as u8);
        true
    }
}

/// Signe un message avec la clé secrète Dilithium
///
/// # Arguments
/// * `sig` - Tableau pour stocker la signature
/// * `msg` - Message à signer
/// * `sk` - Clé secrète
///
/// # Retour
/// `true` si la signature a réussi, `false` sinon
pub fn dilithium_sign(sig: &mut [u8; DILITHIUM_BYTES], msg: &[u8], sk: &[u8; DILITHIUM_SECRETKEYBYTES]) -> bool {
    #[cfg(not(test))]
    {
        extern "C" {
            fn crypto_sign_signature(sig: *mut u8, m: *const u8, mlen: usize, sk: *const u8) -> i32;
        }
        
        let result = unsafe { crypto_sign_signature(sig.as_mut_ptr(), msg.as_ptr(), msg.len(), sk.as_ptr()) };
        result == 0
    }
    
    #[cfg(test)]
    {
        // Simulation pour les tests
        sig.iter_mut().enumerate().for_each(|(i, b)| *b = (i % 256) as u8);
        true
    }
}

/// Vérifie une signature avec la clé publique Dilithium
///
/// # Arguments
/// * `sig` - Signature à vérifier
/// * `msg` - Message signé
/// * `pk` - Clé publique
///
/// # Retour
/// `true` si la signature est valide, `false` sinon
pub fn dilithium_verify(sig: &[u8; DILITHIUM_BYTES], msg: &[u8], pk: &[u8; DILITHIUM_PUBLICKEYBYTES]) -> bool {
    #[cfg(not(test))]
    {
        extern "C" {
            fn crypto_sign_verify(sig: *const u8, m: *const u8, mlen: usize, pk: *const u8) -> i32;
        }
        
        let result = unsafe { crypto_sign_verify(sig.as_ptr(), msg.as_ptr(), msg.len(), pk.as_ptr()) };
        result == 0
    }
    
    #[cfg(test)]
    {
        // Simulation simplifiée pour les tests
        true
    }
}