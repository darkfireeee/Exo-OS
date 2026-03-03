//! Traits et types de base pour le découpage en chunks (no_std).
//!
//! Définit l'interface commune à tous les algorithmes de chunking (CDC, fixe…)
//! ainsi que les types partagés : `DedupChunk`, `ChunkBoundary`, `ChunkStats`.
//!
//! RECUR-01 : aucune récursion.
//! OOM-02   : try_reserve sur tous les Vec.
//! ARITH-02 : checked/saturating/wrapping sur toutes les arithmétiques.

#![allow(dead_code)]

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille minimale absolue d'un chunk (1 KiB).
pub const CHUNK_MIN_SIZE: usize = 1024;
/// Taille cible d'un chunk CDC (8 KiB).
pub const CHUNK_TARGET_SIZE: usize = 8192;
/// Taille maximale absolue d'un chunk (64 KiB).
pub const CHUNK_MAX_SIZE: usize = 65536;
/// Alignement recommandé pour les chunks (512 B, secteur disque).
pub const CHUNK_ALIGN: usize = 512;
/// Nombre maximum de chunks par blob (limite OOM-02).
pub const CHUNK_MAX_PER_BLOB: usize = 16384;

// ─────────────────────────────────────────────────────────────────────────────
// ChunkBoundary — frontière d'un chunk dans le flux de données
// ─────────────────────────────────────────────────────────────────────────────

/// Décrit la frontière d'un chunk dans un flux de données.
///
/// `offset` est l'octet de début dans le flux original.
/// `length` est la taille du chunk en octets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkBoundary {
    /// Offset de début (en octets) dans le flux.
    pub offset: u64,
    /// Longueur du chunk (en octets).
    pub length: u32,
}

impl ChunkBoundary {
    /// Crée une frontière après vérification des invariants.
    ///
    /// ARITH-02 : checked_add.
    pub fn new(offset: u64, length: u32) -> ExofsResult<Self> {
        if length == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if length as usize > CHUNK_MAX_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        // Vérifie que offset + length ne déborde pas.
        offset.checked_add(length as u64).ok_or(ExofsError::OffsetOverflow)?;
        Ok(Self { offset, length })
    }

    /// Retourne l'offset de fin exclusif (= offset + length).
    ///
    /// ARITH-02 : checked_add.
    pub fn end_offset(&self) -> ExofsResult<u64> {
        self.offset.checked_add(self.length as u64).ok_or(ExofsError::OffsetOverflow)
    }

    /// Retourne `true` si ce chunk contient l'offset donné.
    pub fn contains_offset(&self, off: u64) -> bool {
        off >= self.offset && (off - self.offset) < self.length as u64
    }

    /// Retourne `true` si ce chunk chevauche `other`.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.offset < other.offset.saturating_add(other.length as u64)
            && other.offset < self.offset.saturating_add(self.length as u64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupChunk — chunk extrait avec empreinte
// ─────────────────────────────────────────────────────────────────────────────

/// Un chunk de données extrait d'un blob, avec ses métadonnées.
#[derive(Debug, Clone)]
pub struct DedupChunk {
    /// Position et taille dans le flux original.
    pub boundary: ChunkBoundary,
    /// Empreinte Blake3 du contenu du chunk.
    pub blake3:   [u8; 32],
    /// Hash rapide (XxHash64 ou FNV) pour filtrage préliminaire.
    pub fast_hash: u64,
    /// Contenu brut du chunk (peut être vide si on n'a que les métadonnées).
    pub data:     Vec<u8>,
}

impl DedupChunk {
    /// Crée un chunk avec données.
    ///
    /// OOM-02 : try_reserve.
    pub fn new(boundary: ChunkBoundary, blake3: [u8; 32], fast_hash: u64, raw: &[u8])
        -> ExofsResult<Self>
    {
        if raw.len() != boundary.length as usize {
            return Err(ExofsError::InvalidArgument);
        }
        let mut data: Vec<u8> = Vec::new();
        data.try_reserve(raw.len()).map_err(|_| ExofsError::NoMemory)?;
        data.extend_from_slice(raw);
        Ok(Self { boundary, blake3, fast_hash, data })
    }

    /// Crée un chunk sans données (métadonnées seules).
    pub fn metadata_only(boundary: ChunkBoundary, blake3: [u8; 32], fast_hash: u64)
        -> Self
    {
        Self { boundary, blake3, fast_hash, data: Vec::new() }
    }

    /// Retourne `true` si les données sont présentes.
    pub fn has_data(&self) -> bool { !self.data.is_empty() }

    /// Taille déclarée du chunk.
    pub fn len(&self) -> u32 { self.boundary.length }

    /// Vérifie que la taille des données correspond à `boundary.length`.
    pub fn is_consistent(&self) -> bool {
        self.data.is_empty() || self.data.len() == self.boundary.length as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkKind — catégorie d'un chunk
// ─────────────────────────────────────────────────────────────────────────────

/// Catégorie fonctionnelle d'un chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// Chunk de données ordinaire.
    Data,
    /// Chunk de métadonnées (en-tête de blob, index de fichier…).
    Metadata,
    /// Chunk de démarrage (début de blob).
    Prologue,
    /// Chunk de fin (fin de blob).
    Epilogue,
}

impl ChunkKind {
    /// Retourne `true` si ce type de chunk peut être dédupliqué.
    pub fn is_deduplicable(self) -> bool {
        matches!(self, Self::Data)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chunker — trait commun
// ─────────────────────────────────────────────────────────────────────────────

/// Interface commune à tous les algorithmes de chunking.
///
/// RECUR-01 : les implémentations ne doivent pas utiliser de récursion.
pub trait Chunker {
    /// Découpe `data` en une liste de `DedupChunk`.
    ///
    /// OOM-02 : les implémentations doivent utiliser `try_reserve`.
    fn chunk(&self, data: &[u8]) -> ExofsResult<Vec<DedupChunk>>;

    /// Retourne les frontières de chunks sans extraire les données.
    fn boundaries(&self, data: &[u8]) -> ExofsResult<Vec<ChunkBoundary>>;

    /// Taille minimale de chunk supportée par cet algorithme.
    fn min_chunk_size(&self) -> usize;

    /// Taille maximale de chunk supportée par cet algorithme.
    fn max_chunk_size(&self) -> usize;

    /// Retourne le nombre de chunks qui seraient produits pour `data`.
    ///
    /// ARITH-02 : checked_add dans le comptage.
    fn count_chunks(&self, data: &[u8]) -> ExofsResult<usize> {
        Ok(self.boundaries(data)?.len())
    }

    /// Retourne `true` si `data` dépasse `CHUNK_MAX_PER_BLOB` chunks potentiels.
    fn would_exceed_limit(&self, data: &[u8]) -> ExofsResult<bool> {
        Ok(self.count_chunks(data)? > CHUNK_MAX_PER_BLOB)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkStats — statistiques de découpage
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques de découpage produites pour un blob.
#[derive(Debug, Clone, Default)]
pub struct ChunkStats {
    /// Nombre total de chunks produits.
    pub total_chunks:    usize,
    /// Taille minimale observée.
    pub min_chunk_size:  u32,
    /// Taille maximale observée.
    pub max_chunk_size:  u32,
    /// Taille moyenne (approximée, entière).
    pub avg_chunk_size:  u32,
    /// Taille totale des données découpées.
    pub total_bytes:     u64,
}

impl ChunkStats {
    /// Calcule des statistiques depuis une liste de frontières.
    ///
    /// ARITH-02 : checked_add pour total_bytes.
    pub fn from_boundaries(bounds: &[ChunkBoundary]) -> ExofsResult<Self> {
        if bounds.is_empty() {
            return Ok(Self::default());
        }
        let mut min_s: u32 = u32::MAX;
        let mut max_s: u32 = 0;
        let mut total_bytes: u64 = 0;
        for b in bounds {
            if b.length < min_s { min_s = b.length; }
            if b.length > max_s { max_s = b.length; }
            total_bytes = total_bytes.checked_add(b.length as u64)
                .ok_or(ExofsError::OffsetOverflow)?;
        }
        let avg = total_bytes.checked_div(bounds.len() as u64).unwrap_or(0);
        Ok(Self {
            total_chunks:   bounds.len(),
            min_chunk_size: min_s,
            max_chunk_size: max_s,
            avg_chunk_size: avg as u32,
            total_bytes,
        })
    }

    /// Calcule des statistiques depuis une liste de chunks.
    pub fn from_chunks(chunks: &[DedupChunk]) -> ExofsResult<Self> {
        let bounds: Vec<ChunkBoundary> = chunks.iter().map(|c| c.boundary).collect();
        Self::from_boundaries(&bounds)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkList — liste de chunks avec accès indexé
// ─────────────────────────────────────────────────────────────────────────────

/// Conteneur de chunks avec accès indexé et recherche par offset.
pub struct ChunkList {
    chunks: Vec<DedupChunk>,
}

impl ChunkList {
    /// Crée une liste vide.
    pub fn new() -> Self { Self { chunks: Vec::new() } }

    /// Crée depuis un vecteur existant.
    pub fn from_vec(chunks: Vec<DedupChunk>) -> Self { Self { chunks } }

    /// Ajoute un chunk.
    ///
    /// OOM-02 : try_reserve.
    pub fn push(&mut self, chunk: DedupChunk) -> ExofsResult<()> {
        self.chunks.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.chunks.push(chunk);
        Ok(())
    }

    /// Retourne le chunk à l'index `i`.
    pub fn get(&self, i: usize) -> Option<&DedupChunk> { self.chunks.get(i) }

    /// Retourne le chunk contenant l'offset `off`.
    ///
    /// RECUR-01 : recherche linéaire itérative.
    pub fn find_by_offset(&self, off: u64) -> Option<&DedupChunk> {
        self.chunks.iter().find(|c| c.boundary.contains_offset(off))
    }

    /// Nombre de chunks.
    pub fn len(&self) -> usize { self.chunks.len() }

    /// `true` si vide.
    pub fn is_empty(&self) -> bool { self.chunks.is_empty() }

    /// Itérateur sur les chunks.
    pub fn iter(&self) -> core::slice::Iter<'_, DedupChunk> { self.chunks.iter() }

    /// Statistiques sur la liste.
    pub fn stats(&self) -> ExofsResult<ChunkStats> {
        ChunkStats::from_chunks(&self.chunks)
    }

    /// Retourne les chunks dont la taille est inférieure au minimum.
    ///
    /// OOM-02 : try_reserve.
    pub fn undersized_chunks(&self, min: usize) -> ExofsResult<Vec<usize>> {
        let mut out: Vec<usize> = Vec::new();
        out.try_reserve(self.chunks.len()).map_err(|_| ExofsError::NoMemory)?;
        for (i, c) in self.chunks.iter().enumerate() {
            if (c.boundary.length as usize) < min { out.push(i); }
        }
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Aligne `size` sur `align` (arrondi supérieur).
///
/// ARITH-02 : checked_add, wrapping_sub.
pub fn align_up(size: usize, align: usize) -> ExofsResult<usize> {
    if align == 0 { return Err(ExofsError::InvalidArgument); }
    let mask = align.wrapping_sub(1);
    size.checked_add(mask).map(|v| v & !mask).ok_or(ExofsError::OffsetOverflow)
}

/// Vérifie que les frontières ne se chevauchent pas et couvrent [0, total).
///
/// ARITH-02 / RECUR-01.
pub fn validate_boundaries(bounds: &[ChunkBoundary], total: u64) -> ExofsResult<()> {
    if bounds.is_empty() { return Ok(()); }
    let mut cursor: u64 = 0;
    for b in bounds {
        if b.offset != cursor {
            return Err(ExofsError::CorruptedStructure);
        }
        cursor = cursor.checked_add(b.length as u64).ok_or(ExofsError::OffsetOverflow)?;
    }
    if cursor != total { return Err(ExofsError::CorruptedStructure); }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_chunk_boundary_new_ok() {
        let b = ChunkBoundary::new(0, 4096).unwrap();
        assert_eq!(b.length, 4096);
        assert_eq!(b.end_offset().unwrap(), 4096);
    }

    #[test] fn test_chunk_boundary_zero_size() {
        assert!(ChunkBoundary::new(0, 0).is_err());
    }

    #[test] fn test_chunk_boundary_too_large() {
        assert!(ChunkBoundary::new(0, (CHUNK_MAX_SIZE + 1) as u32).is_err());
    }

    #[test] fn test_chunk_boundary_contains_offset() {
        let b = ChunkBoundary::new(100, 50).unwrap();
        assert!(b.contains_offset(100));
        assert!(b.contains_offset(149));
        assert!(!b.contains_offset(150));
        assert!(!b.contains_offset(99));
    }

    #[test] fn test_chunk_boundary_overlap() {
        let b1 = ChunkBoundary::new(0, 100).unwrap();
        let b2 = ChunkBoundary::new(50, 100).unwrap();
        let b3 = ChunkBoundary::new(100, 100).unwrap();
        assert!(b1.overlaps(&b2));
        assert!(!b1.overlaps(&b3));
    }

    #[test] fn test_dedup_chunk_new_ok() {
        let b = ChunkBoundary::new(0, 4).unwrap();
        let c = DedupChunk::new(b, [0u8; 32], 42, &[1, 2, 3, 4]).unwrap();
        assert!(c.has_data());
        assert!(c.is_consistent());
    }

    #[test] fn test_dedup_chunk_size_mismatch() {
        let b = ChunkBoundary::new(0, 4).unwrap();
        assert!(DedupChunk::new(b, [0u8; 32], 0, &[1, 2, 3]).is_err());
    }

    #[test] fn test_chunk_stats_from_boundaries() {
        let bs = alloc::vec![
            ChunkBoundary::new(0,   100).unwrap(),
            ChunkBoundary::new(100, 200).unwrap(),
            ChunkBoundary::new(300, 150).unwrap(),
        ];
        let s = ChunkStats::from_boundaries(&bs).unwrap();
        assert_eq!(s.total_chunks, 3);
        assert_eq!(s.min_chunk_size, 100);
        assert_eq!(s.max_chunk_size, 200);
        assert_eq!(s.total_bytes, 450);
    }

    #[test] fn test_validate_boundaries_ok() {
        let bs = alloc::vec![
            ChunkBoundary::new(0,  100).unwrap(),
            ChunkBoundary::new(100, 50).unwrap(),
        ];
        assert!(validate_boundaries(&bs, 150).is_ok());
    }

    #[test] fn test_validate_boundaries_gap() {
        let bs = alloc::vec![
            ChunkBoundary::new(0, 100).unwrap(),
            ChunkBoundary::new(110, 50).unwrap(),
        ];
        assert!(validate_boundaries(&bs, 160).is_err());
    }

    #[test] fn test_align_up() {
        assert_eq!(align_up(100, 512).unwrap(), 512);
        assert_eq!(align_up(512, 512).unwrap(), 512);
        assert_eq!(align_up(513, 512).unwrap(), 1024);
    }

    #[test] fn test_chunk_list_find_by_offset() {
        let mut list = ChunkList::new();
        let b = ChunkBoundary::new(0, 10).unwrap();
        list.push(DedupChunk::metadata_only(b, [0u8; 32], 0)).unwrap();
        assert!(list.find_by_offset(5).is_some());
        assert!(list.find_by_offset(10).is_none());
    }

    #[test] fn test_chunk_kind_deduplicable() {
        assert!(ChunkKind::Data.is_deduplicable());
        assert!(!ChunkKind::Metadata.is_deduplicable());
    }
}
