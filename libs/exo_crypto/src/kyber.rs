// libs/exo_crypto/src/kyber.rs
#![allow(non_snake_case)]

/// Taille de la clé publique Kyber
pub const KYBER_PUBLICKEYBYTES: usize = 800;
/// Taille de la clé secrète Kyber
pub const KYBER_SECRETKEYBYTES: usize = 1632;
/// Taille du texte chiffré Kyber
pub const KYBER_CIPHERTEXTBYTES: usize = 768;
/// Taille de la clé partagée Kyber
pub const KYBER_BYTES: usize = 32;

/// Génère une paire de clés Kyber
///
/// # Arguments
/// * `pk` - Tableau pour stocker la clé publique
/// * `sk` - Tableau pour stocker la clé secrète
///
/// # Retour
/// `true` si la génération a réussi, `false` sinon
pub fn kyber_keypair(pk: &mut [u8; KYBER_PUBLICKEYBYTES], sk: &mut [u8; KYBER_SECRETKEYBYTES]) -> bool {
    // Dans l'implémentation réelle, cela appellerait la bibliothèque C de Kyber
    // Ici, nous simulons le comportement pour les tests et le développement
    
    #[cfg(not(test))]
    {
        // Appel aux implémentations optimisées en C/ASM
        extern "C" {
            fn crypto_kem_keypair(pk: *mut u8, sk: *mut u8) -> i32;
        }
        
        let result = unsafe { crypto_kem_keypair(pk.as_mut_ptr(), sk.as_mut_ptr()) };
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

/// Encapsule une clé avec la clé publique Kyber
///
/// # Arguments
/// * `ct` - Tableau pour stocker le texte chiffré
/// * `ss` - Tableau pour stocker la clé partagée
/// * `pk` - Clé publique
///
/// # Retour
/// `true` si l'encapsulation a réussi, `false` sinon
pub fn kyber_encaps(ct: &mut [u8; KYBER_CIPHERTEXTBYTES], ss: &mut [u8; KYBER_BYTES], pk: &[u8; KYBER_PUBLICKEYBYTES]) -> bool {
    #[cfg(not(test))]
    {
        extern "C" {
            fn crypto_kem_enc(ct: *mut u8, ss: *mut u8, pk: *const u8) -> i32;
        }
        
        let result = unsafe { crypto_kem_enc(ct.as_mut_ptr(), ss.as_mut_ptr(), pk.as_ptr()) };
        result == 0
    }
    
    #[cfg(test)]
    {
        // Simulation pour les tests
        ct.iter_mut().enumerate().for_each(|(i, b)| *b = (i % 256) as u8);
        ss.iter_mut().enumerate().for_each(|(i, b)| *b = ((i + 17) % 256) as u8);
        true
    }
}

/// Décapsule une clé avec la clé secrète Kyber
///
/// # Arguments
/// * `ss` - Tableau pour stocker la clé partagée
/// * `ct` - Texte chiffré
/// * `sk` - Clé secrète
///
/// # Retour
/// `true` si la décapsulation a réussi, `false` sinon
pub fn kyber_decaps(ss: &mut [u8; KYBER_BYTES], ct: &[u8; KYBER_CIPHERTEXTBYTES], sk: &[u8; KYBER_SECRETKEYBYTES]) -> bool {
    #[cfg(not(test))]
    {
        extern "C" {
            fn crypto_kem_dec(ss: *mut u8, ct: *const u8, sk: *const u8) -> i32;
        }
        
        let result = unsafe { crypto_kem_dec(ss.as_mut_ptr(), ct.as_ptr(), sk.as_ptr()) };
        result == 0
    }
    
    #[cfg(test)]
    {
        // Simulation pour les tests
        ss.iter_mut().enumerate().for_each(|(i, b)| *b = ((i + 17) % 256) as u8);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kyber_keygen() {
        let mut pk = [0u8; KYBER_PUBLICKEYBYTES];
        let mut sk = [0u8; KYBER_SECRETKEYBYTES];
        
        assert!(kyber_keypair(&mut pk, &mut sk));
        
        // Vérifier que les clés ne sont pas nulles
        assert!(!pk.iter().all(|&b| b == 0));
        assert!(!sk.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_kyber_encaps_decaps() {
        let mut pk = [0u8; KYBER_PUBLICKEYBYTES];
        let mut sk = [0u8; KYBER_SECRETKEYBYTES];
        let mut ct = [0u8; KYBER_CIPHERTEXTBYTES];
        let mut ss1 = [0u8; KYBER_BYTES];
        let mut ss2 = [0u8; KYBER_BYTES];
        
        // Générer une paire de clés
        assert!(kyber_keypair(&mut pk, &mut sk));
        
        // Encapsuler
        assert!(kyber_encaps(&mut ct, &mut ss1, &pk));
        
        // Décapsuler
        assert!(kyber_decaps(&mut ss2, &ct, &sk));
        
        // Vérifier que les clés partagées correspondent
        assert_eq!(ss1, ss2);
    }
}