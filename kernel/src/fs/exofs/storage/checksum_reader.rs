// kernel/src/fs/exofs/storage/checksum_reader.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Lecture et vérification de checksum Blake3 — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// ChecksumReader lit un buffer trame `data || ChecksumTag`, vérifie l'intégrité
// via Blake3 et expose la tranche de données nette si le hash est valide.
//
// Règles ExoFS :
// - HDR-03  : magic + checksum vérifiés AVANT tout accès au payload.
// - HASH-02 : la vérification porte sur les données BRUTES (non compressées).

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::blob_id::blake3_hash;
use crate::fs::exofs::storage::checksum_writer::{
    ChecksumTag, CHECKSUM_TAG_LEN, CHECKSUM_MAGIC,
    split_framed, verify_checksum,
};

// ─────────────────────────────────────────────────────────────────────────────
// VerifyResult — résultat de la vérification
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VerifyResult {
    /// Données brutes vérifiées.
    pub data:          Vec<u8>,
    /// Hash Blake3 extrait de la balise.
    pub stored_hash:   [u8; 32],
    /// Hash recalculé sur les données.
    pub computed_hash: [u8; 32],
    /// Nombre d'octets de données (hors balise).
    pub data_bytes:    u64,
}

impl VerifyResult {
    pub fn is_valid(&self) -> bool {
        self.stored_hash == self.computed_hash
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumReader
// ─────────────────────────────────────────────────────────────────────────────

/// Lit et vérifie un buffer trame `data || ChecksumTag`.
///
/// HDR-03 : avant tout accès au payload, `magic` et `hash` sont validés.
pub struct ChecksumReader<'a> {
    framed: &'a [u8],
}

impl<'a> ChecksumReader<'a> {
    /// Crée un lecteur sur un buffer trame.
    /// HDR-03 : le magic est immédiatement vérifié.
    pub fn new(framed: &'a [u8]) -> ExofsResult<Self> {
        if framed.len() < CHECKSUM_TAG_LEN {
            return Err(ExofsError::InvalidSize);
        }
        // HDR-03 : lire le magic AVANT de toucher aux données.
        let tag_start = framed.len() - CHECKSUM_TAG_LEN;
        let magic = u32::from_le_bytes([
            framed[tag_start],
            framed[tag_start + 1],
            framed[tag_start + 2],
            framed[tag_start + 3],
        ]);
        if magic != CHECKSUM_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        Ok(Self { framed })
    }

    // ── Accès aux données vérifiées ──────────────────────────────────────────

    /// Slice sur les données (sans balise).
    /// Renvoie une erreur si le checksum ne correspond pas.
    ///
    /// # Règle HDR-03 : cette fonction vérifie d'abord le hash avant de
    /// retourner la slice de données.
    pub fn data_verified(&self) -> ExofsResult<&[u8]> {
        let (data, tag)  = split_framed(self.framed)?;
        let computed     = blake3_hash(data);
        if computed != tag.hash {
            return Err(ExofsError::ChecksumMismatch);
        }
        Ok(data)
    }

    /// Retourne un VerifyResult complet.
    pub fn verify(&self) -> ExofsResult<VerifyResult> {
        let (data_slice, tag) = split_framed(self.framed)?;
        let computed          = blake3_hash(data_slice);

        let mut data: Vec<u8> = Vec::new();
        data.try_reserve(data_slice.len()).map_err(|_| ExofsError::NoMemory)?;
        data.extend_from_slice(data_slice);

        Ok(VerifyResult {
            data,
            stored_hash:   tag.hash,
            computed_hash: computed,
            data_bytes:    data_slice.len() as u64,
        })
    }

    /// Slice sur les données SANS vérifier le checksum.
    /// À utiliser uniquement si on a déjà appelé `data_verified()`.
    pub fn data_unchecked(&self) -> &[u8] {
        let tag_start = self.framed.len() - CHECKSUM_TAG_LEN;
        &self.framed[..tag_start]
    }

    /// Extrait la balise.
    pub fn tag(&self) -> ExofsResult<ChecksumTag> {
        let tag_start = self.framed.len() - CHECKSUM_TAG_LEN;
        ChecksumTag::from_bytes(&self.framed[tag_start..])
    }

    pub fn framed_len(&self) -> usize { self.framed.len() }
    pub fn data_len(&self)   -> usize { self.framed.len() - CHECKSUM_TAG_LEN }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumStreamVerifier — vérification sur un flux de chunks
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie un flux de chunks par rapport à un hash Blake3 attendu.
///
/// Utile pour vérifier une lecture en plusieurs passes sans assembler le
/// buffer complet en mémoire.
pub struct ChecksumStreamVerifier {
    accumulator: Vec<u8>,
    expected:    [u8; 32],
    finalized:   bool,
}

impl ChecksumStreamVerifier {
    pub fn new(expected: [u8; 32]) -> Self {
        Self { accumulator: Vec::new(), expected, finalized: false }
    }

    /// Ajoute un chunk de données au flux.
    pub fn feed(&mut self, data: &[u8]) -> ExofsResult<()> {
        if self.finalized { return Err(ExofsError::InvalidState); }
        self.accumulator.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        self.accumulator.extend_from_slice(data);
        Ok(())
    }

    /// Finalise la vérification.
    /// Renvoie `true` si le hash recalculé correspond à `expected`.
    pub fn finalize(&mut self) -> ExofsResult<bool> {
        if self.finalized { return Err(ExofsError::InvalidState); }
        self.finalized = true;
        let got = blake3_hash(&self.accumulator);
        Ok(got == self.expected)
    }

    pub fn bytes_fed(&self) -> u64 { self.accumulator.len() as u64 }
    pub fn expected(&self)  -> &[u8; 32] { &self.expected }
    pub fn is_finalized(&self) -> bool { self.finalized }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires (ré-exportées pour la commodité)
// ─────────────────────────────────────────────────────────────────────────────

/// Lit un buffer trame et renvoie seulement les données si valides.
///
/// Combine `ChecksumReader::new()` + `data_verified()`.
/// HDR-03 : magic puis hash vérifiés avant de retourner.
pub fn read_and_verify(framed: &[u8]) -> ExofsResult<Vec<u8>> {
    let reader = ChecksumReader::new(framed)?;
    let data   = reader.data_verified()?;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
    out.extend_from_slice(data);
    Ok(out)
}

/// Vérifie un buffer trame : retourne `true` si intact, `false` si corrompu.
pub fn is_integrity_ok(framed: &[u8]) -> bool {
    match ChecksumReader::new(framed) {
        Ok(reader) => reader.data_verified().is_ok(),
        Err(_)     => false,
    }
}

/// Extrait le hash stocké dans une trame sans vérifier les données.
pub fn extract_hash(framed: &[u8]) -> ExofsResult<[u8; 32]> {
    let reader = ChecksumReader::new(framed)?;
    let tag    = reader.tag()?;
    Ok(tag.hash)
}

/// Vérifie qu'un `data` brut correspond à un `stored_hash` connu.
pub fn verify_raw(data: &[u8], stored_hash: &[u8; 32]) -> bool {
    verify_checksum(data, stored_hash)
}

/// Vérifie que les deux tranches ont le même Blake3.
pub fn hashes_match(a: &[u8], b: &[u8]) -> bool {
    blake3_hash(a) == blake3_hash(b)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::storage::checksum_writer::{ChecksumWriter, append_checksum};

    fn make_framed(data: &[u8]) -> Vec<u8> {
        let mut w = ChecksumWriter::new();
        w.write(data).unwrap();
        let r = w.finalize().unwrap();
        r.framed().unwrap()
    }

    #[test]
    fn test_reader_valid() {
        let framed = make_framed(b"hello");
        let reader = ChecksumReader::new(&framed).unwrap();
        let data   = reader.data_verified().unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn test_reader_corrupted_magic() {
        let mut framed = make_framed(b"hello");
        // Corrompre le magic.
        let tag_pos = framed.len() - CHECKSUM_TAG_LEN;
        framed[tag_pos] ^= 0xFF;
        assert!(ChecksumReader::new(&framed).is_err());
    }

    #[test]
    fn test_reader_corrupted_data() {
        let mut framed = make_framed(b"hello world");
        framed[0] ^= 0x01; // Corruption dans le payload.
        let reader = ChecksumReader::new(&framed).unwrap();
        assert!(reader.data_verified().is_err());
    }

    #[test]
    fn test_stream_verifier() {
        let data     = b"chunk1chunk2chunk3";
        let expected = blake3_hash(data);
        let mut v    = ChecksumStreamVerifier::new(expected);
        v.feed(b"chunk1").unwrap();
        v.feed(b"chunk2").unwrap();
        v.feed(b"chunk3").unwrap();
        assert!(v.finalize().unwrap());
    }

    #[test]
    fn test_stream_verifier_wrong() {
        let expected = [0u8; 32];
        let mut v    = ChecksumStreamVerifier::new(expected);
        v.feed(b"data").unwrap();
        assert!(!v.finalize().unwrap());
    }

    #[test]
    fn test_read_and_verify() {
        let framed = make_framed(b"ExoFS integrity");
        let data   = read_and_verify(&framed).unwrap();
        assert_eq!(&data, b"ExoFS integrity");
    }

    #[test]
    fn test_extract_hash() {
        let raw    = b"some bytes";
        let framed = make_framed(raw);
        let hash   = extract_hash(&framed).unwrap();
        let expect = blake3_hash(raw);
        assert_eq!(hash, expect);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChecksumValidationReport — rapport de validation multi-blocs
// ─────────────────────────────────────────────────────────────────────────────
use crate::fs::exofs::core::DiskOffset;

#[derive(Debug, Clone)]
pub struct BlockValidationResult {
    pub offset:  DiskOffset,
    pub ok:      bool,
    pub reason:  Option<&'static str>,
}

#[derive(Debug)]
pub struct ChecksumValidationReport {
    pub results:     Vec<BlockValidationResult>,
    pub ok_count:    u64,
    pub fail_count:  u64,
}

impl ChecksumValidationReport {
    pub fn new() -> Self {
        Self { results: Vec::new(), ok_count: 0, fail_count: 0 }
    }

    pub fn add_ok(&mut self, offset: DiskOffset) -> ExofsResult<()> {
        self.results.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.results.push(BlockValidationResult { offset, ok: true, reason: None });
        self.ok_count = self.ok_count.saturating_add(1);
        Ok(())
    }

    pub fn add_fail(&mut self, offset: DiskOffset, reason: &'static str) -> ExofsResult<()> {
        self.results.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.results.push(BlockValidationResult { offset, ok: false, reason: Some(reason) });
        self.fail_count = self.fail_count.saturating_add(1);
        Ok(())
    }

    pub fn all_ok(&self) -> bool { self.fail_count == 0 }
    pub fn total(&self)  -> u64  { self.ok_count.saturating_add(self.fail_count) }

    pub fn error_offsets(&self) -> Vec<DiskOffset> {
        self.results.iter().filter(|r| !r.ok).map(|r| r.offset).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchFrameVerifier — vérifie un lot de trames en une passe
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie un lot de buffers tramés `(offset, framed_data)`.
pub struct BatchFrameVerifier;

impl BatchFrameVerifier {
    pub fn verify_all(
        frames: &[(DiskOffset, &[u8])],
    ) -> ExofsResult<ChecksumValidationReport> {
        let mut report = ChecksumValidationReport::new();

        for (offset, framed) in frames {
            match ChecksumReader::new(framed) {
                Err(_) => {
                    report.add_fail(*offset, "bad_magic")?;
                }
                Ok(reader) => {
                    match reader.data_verified() {
                        Ok(_)  => { report.add_ok(*offset)?; }
                        Err(_) => { report.add_fail(*offset, "checksum_mismatch")?; }
                    }
                }
            }
        }
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ReaderMode — mode de lecture strict vs permissif
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VerifyMode {
    /// Erreur si checksum invalide.
    Strict,
    /// Retourne les données même si checksum invalide (mode forensique).
    Permissive,
}

/// Lit et retourne les données selon le mode de vérification.
pub fn read_with_mode(framed: &[u8], mode: VerifyMode) -> ExofsResult<(Vec<u8>, bool)> {
    if framed.len() < CHECKSUM_TAG_LEN { return Err(ExofsError::InvalidSize); }

    let tag_start = framed.len() - CHECKSUM_TAG_LEN;
    let magic     = u32::from_le_bytes([
        framed[tag_start], framed[tag_start+1], framed[tag_start+2], framed[tag_start+3],
    ]);

    if magic != CHECKSUM_MAGIC {
        return Err(ExofsError::BadMagic);
    }

    let data_slice     = &framed[..tag_start];
    let tag            = ChecksumTag::from_bytes(&framed[tag_start..])?;
    let computed       = blake3_hash(data_slice);
    let valid          = computed == tag.hash;

    if !valid && mode == VerifyMode::Strict {
        return Err(ExofsError::ChecksumMismatch);
    }

    let mut data: Vec<u8> = Vec::new();
    data.try_reserve(data_slice.len()).map_err(|_| ExofsError::NoMemory)?;
    data.extend_from_slice(data_slice);

    Ok((data, valid))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extra {
    use super::*;
    use crate::fs::exofs::storage::checksum_writer::ChecksumWriter;

    fn make_framed(data: &[u8]) -> Vec<u8> {
        let mut w = ChecksumWriter::new();
        w.write(data).unwrap();
        w.finalize().unwrap().framed().unwrap()
    }

    #[test]
    fn test_batch_verifier_all_ok() {
        let f1 = make_framed(b"block1");
        let f2 = make_framed(b"block2");
        let frames = vec![
            (DiskOffset(0),    f1.as_slice()),
            (DiskOffset(4096), f2.as_slice()),
        ];
        let report = BatchFrameVerifier::verify_all(&frames).unwrap();
        assert!(report.all_ok());
        assert_eq!(report.total(), 2);
    }

    #[test]
    fn test_batch_verifier_one_fail() {
        let mut f1 = make_framed(b"block1");
        f1[0] ^= 0xFF; // Corruption.
        let f2 = make_framed(b"block2");
        let frames = vec![
            (DiskOffset(0),    f1.as_slice()),
            (DiskOffset(4096), f2.as_slice()),
        ];
        let report = BatchFrameVerifier::verify_all(&frames).unwrap();
        assert!(!report.all_ok());
        assert_eq!(report.fail_count, 1);
    }

    #[test]
    fn test_read_with_mode_strict() {
        let framed = make_framed(b"strict");
        let (data, valid) = read_with_mode(&framed, VerifyMode::Strict).unwrap();
        assert!(valid);
        assert_eq!(&data, b"strict");
    }

    #[test]
    fn test_read_with_mode_permissive_corrupt() {
        let mut framed = make_framed(b"permissive test");
        framed[0] ^= 0x01;
        let (_, valid) = read_with_mode(&framed, VerifyMode::Permissive).unwrap();
        assert!(!valid);
    }

    #[test]
    fn test_read_with_mode_strict_corrupt() {
        let mut framed = make_framed(b"strict test");
        framed[0] ^= 0x01;
        assert!(read_with_mode(&framed, VerifyMode::Strict).is_err());
    }
}
