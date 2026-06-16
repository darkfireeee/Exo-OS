//! volume_secret.rs — Secret de volume ExoFS : racine PERSISTANTE du chiffrement
//! des blobs (FIX-F2).
//!
//! ## Problème corrigé
//! Le `BlobId` est un hash **public** du contenu (Blake3). Dériver la clé de
//! chiffrement du seul `BlobId` ne fournit **aucune** confidentialité : quiconque
//! connaît le contenu (ou le chemin → `BlobId` via PathIndex) recalcule la clé.
//!
//! ## Correction
//! On incorpore une **clé de volume secrète**, installée au montage
//! (`set_volume_key`), déwrappée depuis le superblock via une KEK
//! (passphrase Argon2id / clé scellée TPM/Secure Boot). La clé de chiffrement de
//! chaque blob devient `HKDF-BLAKE3(ikm = volume_key, info = blob_id)`.
//!
//! ## Posture anti « fausse sécurité »
//! Tant qu'**aucune** clé de volume n'est installée, `volume_key()` renvoie `None`
//! et le pipeline de chiffrement **refuse** d'opérer (erreur explicite) plutôt que
//! de fabriquer une clé dérivable par l'attaquant. En `cfg(test)`, une clé
//! déterministe est fournie pour permettre les round-trips de chiffrement.
//!
//! > L'installation de la clé au montage (source de la KEK) est une décision
//! > d'architecture documentée dans `docs/SECURITE/AUDIT-100-PERCENT.md` (F1/F2).

use spin::Once;

/// Clé de volume active (installée au montage en mode chiffré).
static VOLUME_KEY: Once<[u8; 32]> = Once::new();

/// Installe la clé de volume déwrappée. Idempotent : le premier appel gagne
/// (le superblock n'est monté qu'une fois).
pub fn set_volume_key(key: [u8; 32]) {
    VOLUME_KEY.call_once(|| key);
}

/// Retourne la clé de volume si elle est installée, sinon `None`.
///
/// `None` signifie : volume non monté en mode chiffré → le chiffrement-at-rest
/// est honnêtement **inactif** (pas de fausse sécurité).
#[inline]
pub fn volume_key() -> Option<[u8; 32]> {
    #[cfg(test)]
    {
        // En tests : clé déterministe pour les round-trips encrypt/decrypt.
        return Some(VOLUME_KEY.get().copied().unwrap_or([0x7Au8; 32]));
    }
    #[cfg(not(test))]
    {
        VOLUME_KEY.get().copied()
    }
}

/// Vrai si une clé de volume RÉELLE a été installée (hors défaut de test).
#[inline]
pub fn is_installed() -> bool {
    VOLUME_KEY.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_key_present_in_tests() {
        // Le défaut de test garantit une clé déterministe.
        assert!(volume_key().is_some());
        assert_eq!(volume_key().unwrap(), [0x7Au8; 32]);
    }
}
