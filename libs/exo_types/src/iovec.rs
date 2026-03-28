// libs/exo-types/src/iovec.rs
//
// Fichier : libs/exo_types/src/iovec.rs
// Rôle    : IoVec — vecteur I/O pour readv/writev — GI-01 Étape 7.
//
// INVARIANTS :
//   - CORR-45 : #[repr(C, align(8))] + assert taille/alignement compile-time.
//   - IoVec.base = adresse Ring 3 → copy_from_user() OBLIGATOIRE avant usage.
//   - validate_iovec_array() doit être appelée sur tout tableau venant de Ring 3.
//
// SOURCE DE VÉRITÉ :
//   ExoFS_Translation_Layer_v5_FINAL.md §1.1, ExoOS_Corrections_08 CORR-45

/// Vecteur I/O pour `readv`/`writev` — ABI Linux exacte.
///
/// **CORR-45** : Aligné sur 8B avec assertions compile-time.
///
/// ❌ ERREUR CRITIQUE : Utiliser `base` directement sans `copy_from_user()`.
///    `base` est une adresse Ring 3 (userspace) — la déréférencer directement
///    depuis Ring 0 = accès non-contrôlé, potentielle escalade de privilège.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default)]
pub struct IoVec {
    /// Adresse Ring 3 — DOIT être validée par `copy_from_user()` avant usage.
    pub base: u64,
    /// Longueur en bytes.
    pub len:  u64,
}

// Assertions ABI compile-time (CORR-45)
const _: () = assert!(core::mem::size_of::<IoVec>() == 16);
const _: () = assert!(core::mem::align_of::<IoVec>() == 8);

impl IoVec {
    /// Maximum de vecteurs autorisés dans un appel readv/writev (limite POSIX compatible).
    pub const MAX_COUNT: usize = 1024;

    /// Maximum de bytes total dans un appel readv/writev.
    pub const MAX_TOTAL_BYTES: u64 = 0x7FFF_FFFF; // 2GB - 1

    /// Valide un slice d'`IoVec` venant de userspace.
    ///
    /// Vérifie :
    /// - `count <= MAX_COUNT`
    /// - Chaque `len` ne provoque pas d'overflow sur `base + len`
    /// - Total des `len` ne dépasse pas `MAX_TOTAL_BYTES`
    /// - `base` est dans l'espace canonique Ring 3 (< 0x8000_0000_0000)
    ///
    /// **NE vérifie PAS** l'accessibilité réelle des pages (fait par `copy_from_user()`).
    pub fn validate_array(iov: &[IoVec]) -> Result<u64, IoVecError> {
        if iov.len() > Self::MAX_COUNT {
            return Err(IoVecError::TooManyVectors);
        }

        let mut total: u64 = 0;
        for (i, v) in iov.iter().enumerate() {
            // Adresse de base dans l'espace Ring 3 canonique (limite x86_64 userspace)
            if v.base > 0x0000_7FFF_FFFF_FFFF {
                return Err(IoVecError::InvalidAddress { index: i });
            }
            // Pas d'overflow arithmétique sur base + len
            v.base.checked_add(v.len)
                .ok_or(IoVecError::AddressOverflow { index: i })?;
            // Accumulation du total
            total = total.saturating_add(v.len);
            if total > Self::MAX_TOTAL_BYTES {
                return Err(IoVecError::TotalLengthExceeded);
            }
        }
        Ok(total)
    }
}

/// Erreurs de validation IoVec.
#[derive(Debug)]
pub enum IoVecError {
    /// Le tableau dépasse `IOV_MAX` (1024 entrées).
    TooManyVectors,
    /// L'adresse de base du vecteur `index` est nulle ou non alignée.
    InvalidAddress {
        /// Indice du vecteur invalide.
        index: usize
    },
    /// Addition `base + len` déborde un `u64`.
    AddressOverflow {
        /// Indice du vecteur en débordement.
        index: usize
    },
    /// La somme des `len` dépasse `usize::MAX`.
    TotalLengthExceeded,
}
