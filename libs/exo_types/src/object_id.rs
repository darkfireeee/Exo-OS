// libs/exo-types/src/object_id.rs
//
// Fichier : libs/exo_types/src/object_id.rs
// Rôle    : ObjectId opaque — identifiant global ExoFS — GI-01 Étape 3.
//
// INVARIANTS :
//   - SRV-02 : Aucun import blake3 ici. Le hash est calculé par crypto_server.
//   - SRV-04 : ObjectId calculé uniquement dans crypto_server/hash.rs via IPC.
//   - CORR-07 : Exception is_valid() pour ZERO_BLOB_ID_4K (hash Blake3, pas format standard).
//
// FORMAT STANDARD :
//   bytes[0..8] = compteur u64 little-endian
//   bytes[8..32] = zéro (padding)
//   EXCEPTION : ZERO_BLOB_ID_4K est un hash Blake3, pas ce format.
//
// SÉCURITÉ ISR : Copy — utilisable sans allocation.
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Architecture_v7.md §1.2, ExoOS_Kernel_Types_v10.md,
//   ExoFS_Translation_Layer_v5_FINAL.md §1.1, GI-01_Types_TCB_SSR.md §4

/// Identifiant d'objet ExoFS — 32 bytes opaques.
///
/// **Format standard** : `bytes[0..8]` = compteur `u64` LE, `bytes[8..32]` = zéro.
/// **Exception** : [`crate::constants::ZERO_BLOB_ID_4K`] est un hash Blake3 pur.
///
/// ❌ ERREURS GRAVES :
///   1. Calculer un hash blake3 ici → viole SRV-02 et SRV-04.
///   2. Appeler `is_valid()` sur ZERO_BLOB_ID_4K sans connaître l'exception → faux négatif.
///   3. Passer ZERO_BLOB_ID_4K à `blob_refcount::increment()` → refcount virtuel infini corrompu.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    /// ObjectId « zéro » — invalide, utilisé comme sentinelle.
    pub const ZERO: Self = ObjectId([0u8; 32]);

    /// Crée un `ObjectId` depuis un compteur monotone (format standard).
    ///
    /// `bytes[0..8]` = compteur LE, `bytes[8..32]` = zéro garantis.
    #[inline]
    pub fn from_counter(counter: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&counter.to_le_bytes());
        ObjectId(bytes)
    }

    /// Lit le compteur contenu (format standard uniquement).
    ///
    /// Retourne `None` si l'ObjectId ne suit pas le format standard.
    #[inline]
    pub fn counter(&self) -> Option<u64> {
        if self.is_valid() && *self != crate::constants::ZERO_BLOB_ID_4K {
            Some(u64::from_le_bytes(self.0[0..8].try_into().unwrap()))
        } else {
            None
        }
    }

    /// Vérifie la validité de l'`ObjectId`.
    ///
    /// **CORR-07** : Exception explicite pour `ZERO_BLOB_ID_4K`.
    ///
    /// # Règle de validation
    /// - Si l'ObjectId == `ObjectId::ZERO` → `false` (sentinelle invalide).
    /// - Si l'ObjectId == `ZERO_BLOB_ID_4K` → `true` (hash Blake3 valide).
    /// - Sinon : `bytes[8..32]` doivent tous être zéro (format standard).
    ///
    /// ❌ PIÈGE : `is_valid()` ne garantit PAS l'authenticité (forgeable) —
    ///    utiliser les capabilities pour l'autorisation, pas ce test.
    pub fn is_valid(&self) -> bool {
        // Sentinelle nulle : invalide.
        if *self == Self::ZERO {
            return false;
        }

        // CORR-07 : ZERO_BLOB_ID_4K est un P-Blob valide (hash Blake3, pas format standard)
        if *self == crate::constants::ZERO_BLOB_ID_4K {
            return true;
        }
        // Format standard : bytes[8..32] doivent être zéro
        self.0[8..32].iter().all(|&b| b == 0)
    }

    /// Retourne `true` si c'est l'ObjectId sentinelle zéro (nul).
    #[inline(always)]
    pub fn is_null(&self) -> bool {
        *self == Self::ZERO
    }

    /// Construit depuis un tableau brut de 32 bytes.
    ///
    /// # Safety
    /// L'appelant est responsable que les bytes respectent le contrat ObjectId.
    #[inline(always)]
    pub unsafe fn from_raw(bytes: [u8; 32]) -> Self {
        ObjectId(bytes)
    }
}
