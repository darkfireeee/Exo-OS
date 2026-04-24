// kernel/src/fs/exofs/storage/checksum_writer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Écriture avec somme de contrôle Blake3 — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// ChecksumWriter enveloppe un tampon d'écriture et accumule un hash Blake3
// en flux sur les données brutes. Quand toutes les données ont été fournies,
// `finalize()` renvoie la paire `(data, [u8;32])` prête à persister.
//
// Usage typique :
//   let mut w = ChecksumWriter::new();
//   w.write(chunk1)?;
//   w.write(chunk2)?;
//   let (payload, checksum) = w.finalize()?;
//
// Règles ExoFS :
// - HASH-02  : le checksum porte sur les données BRUTES avant compression.
// - OOM-02   : try_reserve avant tout push.
// - ARITH-02 : checked_add pour byte_count.

use crate::fs::exofs::core::blob_id::blake3_hash;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Blake3State — accumulation incrémentale du hash
// ─────────────────────────────────────────────────────────────────────────────

/// Accumulateur incremental Blake3.
///
/// Comme `no_std` ne permet pas d'utiliser directement la crate blake3,
/// nous tamponnons tous les chunks et calculons le hash sur le buffer complet
/// à la finalisation. Cette approche est correcte car nous contrôlons la
/// taille maximale d'un blob.
struct Blake3State {
    buf: Vec<u8>,
}

impl Blake3State {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn feed(&mut self, data: &[u8]) -> ExofsResult<()> {
        self.buf
            .try_reserve(data.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    fn digest(&self) -> [u8; 32] {
        blake3_hash(&self.buf)
    }

    fn len(&self) -> usize {
        self.buf.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumTag — format de la balise stockée après les données
// ─────────────────────────────────────────────────────────────────────────────

/// Taille de la balise de checksum.
pub const CHECKSUM_TAG_LEN: usize = 36; // 4 magic + 32 hash

/// Magic identifiant la balise de checksum Blake3.
pub const CHECKSUM_MAGIC: u32 = 0x4348_5333; // "CHS3"

/// Balise de checksum à stocker juste après les données d'un blob/objet.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct ChecksumTag {
    pub magic: u32,
    pub hash: [u8; 32],
}

impl ChecksumTag {
    pub fn new(hash: [u8; 32]) -> Self {
        Self {
            magic: CHECKSUM_MAGIC,
            hash,
        }
    }

    pub fn to_bytes(&self) -> [u8; CHECKSUM_TAG_LEN] {
        let mut out = [0u8; CHECKSUM_TAG_LEN];
        out[0..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..36].copy_from_slice(&self.hash);
        out
    }

    pub fn from_bytes(b: &[u8]) -> ExofsResult<Self> {
        if b.len() < CHECKSUM_TAG_LEN {
            return Err(ExofsError::InvalidSize);
        }
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if magic != CHECKSUM_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&b[4..36]);
        Ok(Self { magic, hash })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumWriterState
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WriterState {
    Open,
    Finalized,
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumWriter
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit des données en accumulant un checksum Blake3.
///
/// # Exemple
/// ```
/// let mut w = ChecksumWriter::new();
/// w.write(b"hello")?;
/// w.write(b" world")?;
/// let result = w.finalize()?;
/// // result.data  = b"hello world"
/// // result.tag   = ChecksumTag { magic: 0x4348_5333, hash: [...] }
/// ```
pub struct ChecksumWriter {
    state: Blake3State,
    status: WriterState,
}

/// Résultat de la finalisation.
pub struct ChecksumResult {
    /// Données fournies (sans la balise).
    pub data: Vec<u8>,
    /// Balise de checksum.
    pub tag: ChecksumTag,
    /// Hash brut (copie de tag.hash).
    pub hash: [u8; 32],
    /// Octets totaux écrits (hors balise).
    pub bytes: u64,
}

impl ChecksumResult {
    /// Sérialise `data || tag` en un seul vecteur prêt à persister.
    pub fn framed(&self) -> ExofsResult<Vec<u8>> {
        let total = self
            .data
            .len()
            .checked_add(CHECKSUM_TAG_LEN)
            .ok_or(ExofsError::Overflow)?;
        let mut out: Vec<u8> = Vec::new();
        out.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(&self.data);
        out.extend_from_slice(&self.tag.to_bytes());
        Ok(out)
    }
}

impl ChecksumWriter {
    pub fn new() -> Self {
        Self {
            state: Blake3State::new(),
            status: WriterState::Open,
        }
    }

    /// Crée un writer avec pré-allocation de `capacity` octets.
    pub fn with_capacity(capacity: usize) -> ExofsResult<Self> {
        let mut state = Blake3State::new();
        state
            .buf
            .try_reserve(capacity)
            .map_err(|_| ExofsError::NoMemory)?;
        Ok(Self {
            state,
            status: WriterState::Open,
        })
    }

    /// Ajoute des données au flux.
    ///
    /// # Règle HASH-02 : ces données doivent être **brutes** (non compressées).
    pub fn write(&mut self, data: &[u8]) -> ExofsResult<usize> {
        if self.status == WriterState::Finalized {
            return Err(ExofsError::InvalidState);
        }
        self.state.feed(data)?;
        Ok(data.len())
    }

    /// Ajoute plusieurs morceaux en séquence.
    pub fn write_all(&mut self, chunks: &[&[u8]]) -> ExofsResult<u64> {
        let mut total = 0u64;
        for chunk in chunks {
            let n = self.write(chunk)? as u64;
            total = total.checked_add(n).ok_or(ExofsError::Overflow)?;
        }
        Ok(total)
    }

    /// Finalise le flux et retourne les données avec la balise de checksum.
    pub fn finalize(mut self) -> ExofsResult<ChecksumResult> {
        if self.status == WriterState::Finalized {
            return Err(ExofsError::InvalidState);
        }
        let hash = self.state.digest();
        let tag = ChecksumTag::new(hash);
        let bytes = self.state.len() as u64;
        let data = self.state.buf;
        self.status = WriterState::Finalized;
        Ok(ChecksumResult {
            data,
            tag,
            hash,
            bytes,
        })
    }

    /// Nombre d'octets reçus jusqu'à présent.
    pub fn byte_count(&self) -> u64 {
        self.state.len() as u64
    }

    pub fn is_finalized(&self) -> bool {
        self.status == WriterState::Finalized
    }
}

impl Default for ChecksumWriter {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumAppender — ajoute la balise à un buffer existant en place
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule et appende la balise de checksum à un `Vec<u8>` existant.
///
/// # Règle HASH-02 : `buf` doit contenir les données BRUTES avant compression.
pub fn append_checksum(buf: &mut Vec<u8>) -> ExofsResult<[u8; 32]> {
    let hash = blake3_hash(buf);
    let tag = ChecksumTag::new(hash);
    let raw = tag.to_bytes();
    buf.try_reserve(CHECKSUM_TAG_LEN)
        .map_err(|_| ExofsError::NoMemory)?;
    buf.extend_from_slice(&raw);
    Ok(hash)
}

/// Calcule le checksum de `data` sans modifier le vecteur.
pub fn compute_checksum(data: &[u8]) -> [u8; 32] {
    blake3_hash(data)
}

/// Vérifie que `data` (sans balise) correspond au hash `expected`.
pub fn verify_checksum(data: &[u8], expected: &[u8; 32]) -> bool {
    let got = blake3_hash(data);
    got == *expected
}

/// Séparateur : décompose `framed = data || tag`.
/// Renvoie `(data_slice, tag)`.
pub fn split_framed(framed: &[u8]) -> ExofsResult<(&[u8], ChecksumTag)> {
    if framed.len() < CHECKSUM_TAG_LEN {
        return Err(ExofsError::InvalidSize);
    }
    let data_end = framed.len() - CHECKSUM_TAG_LEN;
    let tag = ChecksumTag::from_bytes(&framed[data_end..])?;
    Ok((&framed[..data_end], tag))
}

/// Vérifie un buffer trame complet (`data || tag`).
pub fn verify_framed(framed: &[u8]) -> ExofsResult<bool> {
    let (data, tag) = split_framed(framed)?;
    Ok(verify_checksum(data, &tag.hash))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_writer_basic() {
        let mut w = ChecksumWriter::new();
        w.write(b"hello").unwrap();
        w.write(b" world").unwrap();
        let r = w.finalize().unwrap();
        assert_eq!(r.bytes, 11);
        assert_eq!(r.data, b"hello world");
        assert_eq!(r.hash, r.tag.hash);
    }

    #[test]
    fn test_framed_roundtrip() {
        let mut w = ChecksumWriter::new();
        w.write(b"ExoFS checksum test data").unwrap();
        let r = w.finalize().unwrap();
        let framed = r.framed().unwrap();
        assert!(verify_framed(&framed).unwrap());
    }

    #[test]
    fn test_append_checksum() {
        let mut buf = b"test".to_vec();
        let hash = append_checksum(&mut buf).unwrap();
        assert_eq!(buf.len(), 4 + CHECKSUM_TAG_LEN);
        assert!(verify_framed(&buf).unwrap());
        let _ = hash;
    }

    #[test]
    fn test_split_framed() {
        let mut w = ChecksumWriter::new();
        w.write(b"payload").unwrap();
        let r = w.finalize().unwrap();
        let framed = r.framed().unwrap();
        let (data, tag) = split_framed(&framed).unwrap();
        assert_eq!(data, b"payload");
        assert_eq!(tag.magic, CHECKSUM_MAGIC);
    }

    #[test]
    fn test_corruption_detected() {
        let mut framed_vec: Vec<u8>;
        {
            let mut w = ChecksumWriter::new();
            w.write(b"sensitive data").unwrap();
            let r = w.finalize().unwrap();
            framed_vec = r.framed().unwrap();
        }
        framed_vec[0] ^= 0xFF; // Corruption.
        assert!(!verify_framed(&framed_vec).unwrap());
    }

    #[test]
    fn test_double_finalize_fails() {
        let w = ChecksumWriter::new();
        let _r = w.finalize().unwrap();
        // La seconde finalisation doit échouer (moved).
        // (Rust guarantit ceci par le système de move, pas besoin d'assertion.)
    }

    #[test]
    fn test_checksum_tag_roundtrip() {
        let hash = [0xABu8; 32];
        let tag = ChecksumTag::new(hash);
        let raw = tag.to_bytes();
        let tag2 = ChecksumTag::from_bytes(&raw).unwrap();
        assert_eq!(tag2.magic, CHECKSUM_MAGIC);
        assert_eq!(tag2.hash, hash);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockChecksumMap — table de checksums par bloc disque
// ─────────────────────────────────────────────────────────────────────────────
use crate::fs::exofs::core::DiskOffset;

/// Entrée dans la table de checksums par bloc.
#[derive(Clone, Debug)]
pub struct BlockChecksumEntry {
    pub offset: DiskOffset,
    pub hash: [u8; 32],
    pub valid: bool,
}

/// Table non-ordonnée de checksums par offset disque.
/// Permet de vérifier l'intégrité de n'importe quel bloc sans le lire
/// depuis l'autre extrémité de la pipeline.
pub struct BlockChecksumMap {
    entries: Vec<BlockChecksumEntry>,
}

impl BlockChecksumMap {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> ExofsResult<Self> {
        let mut e = Vec::new();
        e.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { entries: e })
    }

    /// Enregistre le checksum d'un bloc.
    pub fn record(&mut self, offset: DiskOffset, data: &[u8]) -> ExofsResult<[u8; 32]> {
        let hash = blake3_hash(data);
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        // Mise à jour si déjà présent.
        for e in &mut self.entries {
            if e.offset == offset {
                e.hash = hash;
                e.valid = true;
                return Ok(hash);
            }
        }
        self.entries.push(BlockChecksumEntry {
            offset,
            hash,
            valid: true,
        });
        Ok(hash)
    }

    /// Vérifie un bloc contre le hash enregistré.
    pub fn verify(&self, offset: DiskOffset, data: &[u8]) -> ExofsResult<bool> {
        for e in &self.entries {
            if e.offset == offset {
                if !e.valid {
                    return Err(ExofsError::InvalidState);
                }
                return Ok(blake3_hash(data) == e.hash);
            }
        }
        Err(ExofsError::NotFound)
    }

    /// Invalide un bloc (ex: après écriture).
    pub fn invalidate(&mut self, offset: DiskOffset) {
        for e in &mut self.entries {
            if e.offset == offset {
                e.valid = false;
            }
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
    pub fn valid_count(&self) -> usize {
        self.entries.iter().filter(|e| e.valid).count()
    }
    pub fn invalid_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.valid).count()
    }
    pub fn contains(&self, off: DiskOffset) -> bool {
        self.entries.iter().any(|e| e.offset == off)
    }

    /// Liste des blocs invalides.
    pub fn invalid_offsets(&self) -> Vec<DiskOffset> {
        self.entries
            .iter()
            .filter(|e| !e.valid)
            .map(|e| e.offset)
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumChainer — chaîne de checksums pour les blobs multi-segments
// ─────────────────────────────────────────────────────────────────────────────

/// Chaîne un hash courant avec la donnée suivante pour créer une chaîne
/// d'intégrité : `hash[n] = Blake3(hash[n-1] || data[n])`.
///
/// Utile pour les blobs écrits en plusieurs segments : chaque segment
/// authentifie tous les précédents.
pub struct ChecksumChainer {
    chain_hash: [u8; 32],
    segment: u32,
}

impl ChecksumChainer {
    /// Initialise avec un IV (ex : BlobId du blob en cours).
    pub fn new(iv: [u8; 32]) -> Self {
        Self {
            chain_hash: iv,
            segment: 0,
        }
    }

    /// Chaîne un nouveau segment.
    pub fn feed_segment(&mut self, data: &[u8]) -> ExofsResult<[u8; 32]> {
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(32 + data.len())
            .map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(&self.chain_hash);
        buf.extend_from_slice(data);
        let new_hash = blake3_hash(&buf);
        self.chain_hash = new_hash;
        self.segment = self.segment.wrapping_add(1);
        Ok(new_hash)
    }

    pub fn current_hash(&self) -> [u8; 32] {
        self.chain_hash
    }
    pub fn segment_index(&self) -> u32 {
        self.segment
    }
}

#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_block_checksum_map() {
        let mut m = BlockChecksumMap::new();
        let data = b"block contents";
        let off = DiskOffset(4096);
        m.record(off, data).unwrap();
        assert!(m.verify(off, data).unwrap());
        assert!(!m.verify(off, b"different").unwrap());
    }

    #[test]
    fn test_block_checksum_invalidate() {
        let mut m = BlockChecksumMap::new();
        let off = DiskOffset(0);
        m.record(off, b"data").unwrap();
        m.invalidate(off);
        assert_eq!(m.valid_count(), 0);
        assert_eq!(m.invalid_count(), 1);
    }

    #[test]
    fn test_chainer_deterministic() {
        let iv = [0u8; 32];
        let mut c1 = ChecksumChainer::new(iv);
        let mut c2 = ChecksumChainer::new(iv);
        let h1 = c1.feed_segment(b"same data").unwrap();
        let h2 = c2.feed_segment(b"same data").unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_chainer_diverges_on_different_data() {
        let iv = [0u8; 32];
        let mut c1 = ChecksumChainer::new(iv);
        let mut c2 = ChecksumChainer::new(iv);
        let h1 = c1.feed_segment(b"data_A").unwrap();
        let h2 = c2.feed_segment(b"data_B").unwrap();
        assert_ne!(h1, h2);
    }
}
