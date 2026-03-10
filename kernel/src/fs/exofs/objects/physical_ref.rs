// SPDX-License-Identifier: MIT
// ExoFS — physical_ref.rs
// Référence typée à la ressource physique d'un LogicalObject.
// Règles :
//   REFCNT-01 : le ref-count est dans PhysicalBlobInMemory, pas ici
//   SEC-04    : jamais de contenu secret dans les logs/stats
//   ARITH-02  : checked_add / saturating_*


use core::fmt;
use alloc::sync::Arc;

use crate::fs::exofs::core::{BlobId, DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::objects::inline_data::InlineData;
use crate::fs::exofs::objects::physical_blob::PhysicalBlobInMemory;

// ── PhysicalRef ────────────────────────────────────────────────────────────────

/// Référence à la ressource physique d'un `LogicalObject`.
///
/// Un `LogicalObject` peut stocker ses données de trois façons :
/// - **Blob** : pointeur vers un `PhysicalBlobInMemory` partagé (dédupliqué).
/// - **Inline** : données embarquées directement dans l'objet (< 512 B).
/// - **Empty** : aucune donnée (objet de métadonnées pur, répertoire vide, …).
///
/// La variante `Blob` tient un `Arc<PhysicalBlobInMemory>` ; la durée
/// de vie du P-Blob est donc liée à celle de ce `PhysicalRef`.
#[derive(Clone)]
pub enum PhysicalRef {
    /// Données persistées dans un P-Blob externe (dédupliqué ou non).
    ///
    /// Selon la spec 2.2, la variante `Unique` ou `Shared` est indiquée
    /// via les flags du `LogicalObject`. Ici on unifie les deux dans `Blob`.
    Blob(Arc<PhysicalBlobInMemory>),

    /// Données embarquées dans le `LogicalObject` (< `INLINE_DATA_MAX`).
    Inline(InlineData),

    /// Objet sans données (métadonnées pures, répertoire vide, compteur, …).
    Empty,
}

impl PhysicalRef {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Crée un `PhysicalRef` pointant vers un P-Blob.
    #[inline]
    pub fn from_blob(blob: Arc<PhysicalBlobInMemory>) -> Self {
        Self::Blob(blob)
    }

    /// Crée un `PhysicalRef` inline depuis des données brutes.
    ///
    /// Retourne `ExofsError::InlineTooLarge` si `data` dépasse 512 octets.
    pub fn from_inline_data(data: &[u8]) -> ExofsResult<Self> {
        let id = InlineData::from_slice(data)?;
        Ok(Self::Inline(id))
    }

    /// Crée un `PhysicalRef` vide.
    #[inline]
    pub fn empty() -> Self {
        Self::Empty
    }

    // ── Requêtes ──────────────────────────────────────────────────────────────

    /// Retourne le `BlobId` si la référence est un P-Blob.
    pub fn blob_id(&self) -> Option<BlobId> {
        match self {
            Self::Blob(b) => Some(b.blob_id),
            _             => None,
        }
    }

    /// Retourne l'offset disque si la référence est un P-Blob.
    pub fn disk_offset(&self) -> Option<DiskOffset> {
        match self {
            Self::Blob(b) => Some(b.data_offset),
            _             => None,
        }
    }

    /// Retourne la taille logique des données (en octets).
    ///
    /// - Blob   → `data_size` (données sur disque).
    /// - Inline → longueur des données inline.
    /// - Empty  → 0.
    pub fn size(&self) -> u64 {
        match self {
            Self::Blob(b)   => b.data_size,
            Self::Inline(d) => d.len() as u64,
            Self::Empty     => 0,
        }
    }

    /// Retourne la taille originale (avant compression).
    pub fn original_size(&self) -> u64 {
        match self {
            Self::Blob(b)   => b.original_size,
            Self::Inline(d) => d.len() as u64,
            Self::Empty     => 0,
        }
    }

    /// Retourne `true` si la référence est un P-Blob.
    #[inline]
    pub fn is_blob(&self) -> bool {
        matches!(self, Self::Blob(_))
    }

    /// Retourne `true` si les données sont inline.
    #[inline]
    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Inline(_))
    }

    /// Retourne `true` si l'objet n'a pas de données.
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Retourne le P-Blob sous-jacent, si présent.
    pub fn as_blob(&self) -> Option<&Arc<PhysicalBlobInMemory>> {
        match self {
            Self::Blob(b) => Some(b),
            _             => None,
        }
    }

    /// Retourne les données inline, si présentes.
    pub fn as_inline(&self) -> Option<&InlineData> {
        match self {
            Self::Inline(d) => Some(d),
            _               => None,
        }
    }

    /// Retourne les données inline mutables, si présentes.
    pub fn as_inline_mut(&mut self) -> Option<&mut InlineData> {
        match self {
            Self::Inline(d) => Some(d),
            _               => None,
        }
    }

    // ── Opérations de ref-count (délégation) ─────────────────────────────────

    /// Incrémente le ref-count du P-Blob, si applicable.
    pub fn inc_ref(&self) {
        if let Self::Blob(b) = self {
            b.inc_ref();
        }
    }

    /// Décrémente le ref-count du P-Blob, si applicable.
    ///
    /// **Panic** si le compteur atteint 0 sous sa valeur minimale (REFCNT-01).
    pub fn dec_ref(&self) -> u32 {
        match self {
            Self::Blob(b) => b.dec_ref(),
            _             => 0,
        }
    }

    /// Retourne le ref-count courant.
    pub fn ref_count(&self) -> u32 {
        match self {
            Self::Blob(b) => b.ref_count(),
            _             => 0,
        }
    }

    // ── Vérification ──────────────────────────────────────────────────────────

    /// Vérifie que le contenu `data` correspond au `blob_id` (HASH-01).
    ///
    /// Pour les variants non-Blob, retourne toujours `true` (rien à vérifier).
    pub fn verify_content(&self, data: &[u8]) -> bool {
        match self {
            Self::Blob(b)   => b.verify_content(data),
            Self::Inline(d) => d.ct_eq(data) || d.verify_hash(),
            Self::Empty     => data.is_empty(),
        }
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /// Valide la cohérence de la référence physique.
    pub fn validate(&self) -> ExofsResult<()> {
        match self {
            Self::Blob(b) => {
                if b.data_size == 0 && b.original_size == 0 {
                    return Err(ExofsError::Corrupt);
                }
                Ok(())
            }
            Self::Inline(d) => d.validate(),
            Self::Empty     => Ok(()),
        }
    }

    // ── Conversion ────────────────────────────────────────────────────────────

    /// Tente de convertir un `Inline` vers un `Blob` en fournissant
    /// le blob pré-alloué. Retourne `Err` si la référence n'est pas `Inline`.
    pub fn promote_to_blob(&mut self, blob: Arc<PhysicalBlobInMemory>) -> ExofsResult<()> {
        if !self.is_inline() {
            return Err(ExofsError::InvalidArgument);
        }
        *self = Self::Blob(blob);
        Ok(())
    }

    /// Retourne une description textuelle du type de référence.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Blob(_)   => "blob",
            Self::Inline(_) => "inline",
            Self::Empty     => "empty",
        }
    }
}

// ── Display / Debug ────────────────────────────────────────────────────────────

impl fmt::Display for PhysicalRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(b) => {
                // SEC-04 : on affiche l'offset et la taille, pas le contenu.
                write!(
                    f,
                    "PhysicalRef::Blob {{ offset: {:#x}, size: {}, refs: {} }}",
                    b.data_offset.0,
                    b.data_size,
                    b.ref_count(),
                )
            }
            Self::Inline(d) => {
                write!(f, "PhysicalRef::Inline {{ len: {} }}", d.len())
            }
            Self::Empty => f.write_str("PhysicalRef::Empty"),
        }
    }
}

impl fmt::Debug for PhysicalRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── PhysicalRefStats ───────────────────────────────────────────────────────────

/// Statistiques sur les transitions de `PhysicalRef`.
#[derive(Default, Debug, Clone)]
pub struct PhysicalRefStats {
    /// Nombre de refs créées (all variants).
    pub created:          u64,
    /// Nombre de refs Blob créées.
    pub blob_refs:        u64,
    /// Nombre de refs Inline créées.
    pub inline_refs:      u64,
    /// Nombre de promotions inline→blob.
    pub promotions:       u64,
    /// Nombre d'erreurs de validation.
    pub validate_errors:  u64,
    /// Nombre de vérifications de contenu réussies.
    pub verify_ok:        u64,
    /// Nombre d'erreurs de vérification.
    pub verify_errors:    u64,
}

impl PhysicalRefStats {
    pub const fn new() -> Self {
        Self {
            created:         0,
            blob_refs:       0,
            inline_refs:     0,
            promotions:      0,
            validate_errors: 0,
            verify_ok:       0,
            verify_errors:   0,
        }
    }
}

impl fmt::Display for PhysicalRefStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PhysicalRefStats {{ created: {}, blobs: {}, inline: {}, \
             promotions: {}, verify_ok: {}, verify_err: {} }}",
            self.created,
            self.blob_refs,
            self.inline_refs,
            self.promotions,
            self.verify_ok,
            self.verify_errors,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::EpochId;
    use crate::fs::exofs::objects::physical_blob::CompressionType;

    fn make_blob_ref() -> Arc<PhysicalBlobInMemory> {
        Arc::new(PhysicalBlobInMemory::new(
            DiskOffset(0x10000),
            b"test data",
            9,
            CompressionType::None,
            EpochId(1),
        ))
    }

    #[test]
    fn test_blob_ref_basics() {
        let r = PhysicalRef::from_blob(make_blob_ref());
        assert!(r.is_blob());
        assert!(!r.is_inline());
        assert_eq!(r.size(), 9);
        assert!(r.blob_id().is_some());
    }

    #[test]
    fn test_inline_ref() {
        let r = PhysicalRef::from_inline_data(b"hello").unwrap();
        assert!(r.is_inline());
        assert_eq!(r.size(), 5);
        assert!(r.blob_id().is_none());
    }

    #[test]
    fn test_empty_ref() {
        let r = PhysicalRef::empty();
        assert!(r.is_empty());
        assert_eq!(r.size(), 0);
        assert!(r.verify_content(b""));
    }

    #[test]
    fn test_inline_too_large() {
        let big = [0u8; 513];
        assert!(PhysicalRef::from_inline_data(&big).is_err());
    }

    #[test]
    fn test_dec_ref_via_physical_ref() {
        let b = make_blob_ref();
        let r = PhysicalRef::from_blob(Arc::clone(&b));
        r.inc_ref();
        assert_eq!(b.ref_count(), 2);
        r.dec_ref();
        assert_eq!(b.ref_count(), 1);
    }

    #[test]
    fn test_verify_blob_content() {
        let data = b"verify me";
        let b = Arc::new(PhysicalBlobInMemory::new(
            DiskOffset(0),
            data,
            data.len() as u64,
            CompressionType::None,
            EpochId(1),
        ));
        let r = PhysicalRef::from_blob(b);
        assert!( r.verify_content(data));
        assert!(!r.verify_content(b"wrong"));
    }
}
