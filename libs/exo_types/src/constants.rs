// libs/exo-types/src/constants.rs
//
// Fichier : libs/exo_types/src/constants.rs
// Rôle    : Constantes globales ExoFS et types — GI-01 Étape 8.
//
// INVARIANTS :
//   - SRV-02 / SRV-04 : ZERO_BLOB_ID_4K est pré-calculé, JAMAIS recalculé en Ring 0.
//   - TL-31 : io/reader.rs → memset(0) sans I/O disque si p_blob_id == ZERO_BLOB_ID_4K.
//   - TL-02 : ZERO_BLOB_ID_4K pour pages entières (4096 bytes) UNIQUEMENT.
//   - TL-32 : page partielle → write normal (pas ZERO_BLOB_ID_4K).
//
// SOURCE DE VÉRITÉ :
//   ExoFS_Translation_Layer_v5_FINAL.md §1.1, GI-01_Types_TCB_SSR.md §4

use crate::object_id::ObjectId;

/// Taille d'une page ExoFS — alignée sur la page x86_64 standard.
pub const EXOFS_PAGE_SIZE: usize = 4096;

/// `Blake3([0u8; 4096])` — pré-calculé, codé en dur.
///
/// **Règle TL-31** : Toute page lue avec `p_blob_id == ZERO_BLOB_ID_4K`
/// doit être remplie de zéros (`memset(0)`) **sans aucun accès disque**.
///
/// ❌ ERREURS GRAVES :
///   1. Recalculer en Ring 0 via blake3 → viole SRV-04 (blake3 réservé à crypto_server).
///   2. Passer à `blob_refcount::increment()` → corrupt le système de déduplication.
///   3. Utiliser pour une page partielle (len < EXOFS_PAGE_SIZE) → viole TL-32.
///
/// La valeur a été vérifiée par le test `constants::blake3_zero_4k_correct`
/// (CI host uniquement — pas compilé dans le kernel).
pub const ZERO_BLOB_ID_4K: ObjectId = ObjectId([
    0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6,
    0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdb, 0xc9, 0xab,
    0x14, 0x46, 0x34, 0x66, 0x0a, 0x71, 0x38, 0x5f,
    0x02, 0x28, 0xe7, 0xd7, 0x0b, 0xce, 0xe1, 0x07,
]);

/// Taille maximale d'un nom de service IPC (ServiceName).
pub const SERVICE_NAME_LEN: usize = 64;

/// Taille maximale d'un chemin de fichier (PathBuf).
pub const PATH_BUF_LEN: usize = 512;

// ─── Test de validation (CI host uniquement — jamais dans le kernel) ─────────
#[cfg(test)]
mod tests {
    use super::*;

    /// Vérifie que ZERO_BLOB_ID_4K == Blake3([0u8; 4096]).
    ///
    /// S'exécute côté host avec le crate blake3.
    /// JAMAIS compilé dans le kernel (target x86_64-unknown-none).
    #[test]
    fn blake3_zero_4k_correct() {
        #[cfg(feature = "test-blake3")]
        {
            let input = [0u8; 4096];
            let hash = blake3::hash(&input);
            assert_eq!(
                hash.as_bytes(),
                &ZERO_BLOB_ID_4K.0,
                "ZERO_BLOB_ID_4K ne correspond pas à Blake3([0u8; 4096])"
            );
        }
    }

    #[test]
    fn zero_blob_id_is_valid() {
        // CORR-07 : ZERO_BLOB_ID_4K doit passer is_valid() malgré son format non-standard
        assert!(
            ZERO_BLOB_ID_4K.is_valid(),
            "ZERO_BLOB_ID_4K doit être reconnu valide (exception CORR-07)"
        );
    }

    #[test]
    fn counter_zero_is_invalid() {
        // ObjectId::ZERO est la sentinelle invalide
        assert!(
            !ObjectId::ZERO.is_valid(),
            "ObjectId::ZERO ne doit pas être valide"
        );
    }

    #[test]
    fn from_counter_roundtrip() {
        let oid = ObjectId::from_counter(42);
        assert!(oid.is_valid());
        assert_eq!(oid.counter(), Some(42));
    }
}
