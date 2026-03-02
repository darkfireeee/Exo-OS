// kernel/src/fs/exofs/objects/inline_data.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// InlineData — données embarquées dans l'objet (< 512 octets)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Les petits objets (< INLINE_DATA_MAX = 512 octets par défaut) stockent
// leurs données directement dans le LogicalObject au lieu d'un P-Blob externe.
// Cela évite une allocation heap pour les métadonnées et les petits fichiers.

use crate::fs::exofs::core::{ExofsError, ExofsResult, INLINE_DATA_MAX};

/// Contenu inline d'un LogicalObject (données directement dans l'objet).
///
/// Taille maximale = INLINE_DATA_MAX (512 octets par défaut).
/// Stocké dans un tableau [u8; INLINE_DATA_MAX] + longueur réelle.
pub struct InlineData {
    buf:    [u8; 512],
    len:    usize,
}

impl InlineData {
    /// Crée un InlineData depuis une slice.
    ///
    /// Retourne Err si `data.len() > INLINE_DATA_MAX`.
    pub fn from_slice(data: &[u8]) -> ExofsResult<Self> {
        if data.len() > INLINE_DATA_MAX {
            return Err(ExofsError::InlineTooLarge);
        }
        let mut buf = [0u8; 512];
        buf[..data.len()].copy_from_slice(data);
        Ok(Self { buf, len: data.len() })
    }

    /// Retourne les données inline.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Retourne la longueur réelle.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Vrai si les données inline sont vides.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Met à jour le contenu inline.
    pub fn update(&mut self, data: &[u8]) -> ExofsResult<()> {
        if data.len() > INLINE_DATA_MAX {
            return Err(ExofsError::InlineTooLarge);
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.len = data.len();
        Ok(())
    }
}
