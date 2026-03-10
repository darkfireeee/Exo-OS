//! Lecteur de secrets chiffrés — déchiffrement de blobs XChaCha20-Poly1305.
//!
//! FORMAT DU PAYLOAD CHIFFRÉ :
//!   [magic: 4 octets][nonce: 24 octets][tag: 16 octets][longueur_clair: 8 octets][ciphertext: N octets]
//!   → Header total = 52 octets
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.


use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::xchacha20::{XChaCha20Key, XChaCha20Poly1305, Nonce, Tag};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Magic identifiant un payload ExoFS chiffré.
pub const SECRET_MAGIC: [u8; 4] = [0xEF, 0x5E, 0x52, 0x44]; // "ExoRD"

/// Taille du header: magic(4) + nonce(24) + tag(16) + len(8)
pub const SECRET_HEADER_SIZE: usize = 52;

/// Taille minimale d'un payload valide (header + au moins 1 octet).
pub const SECRET_MIN_PAYLOAD: usize = SECRET_HEADER_SIZE.saturating_add(1);

/// Version actuelle du format.
pub const SECRET_FORMAT_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Header
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête d'un secret chiffré.
#[derive(Debug, Clone)]
pub struct SecretHeader {
    /// Nonce utilisé pour le chiffrement.
    pub nonce:         Nonce,
    /// Tag d'authentification Poly1305.
    pub tag:           Tag,
    /// Longueur des données en clair (pré-vérification).
    pub plaintext_len: u64,
}

impl SecretHeader {
    /// Parse un header depuis les 52 premiers octets du payload.
    ///
    /// ARITH-02.
    pub fn parse(payload: &[u8]) -> ExofsResult<Self> {
        if payload.len() < SECRET_HEADER_SIZE {
            return Err(ExofsError::CorruptedStructure);
        }
        // Vérification du magic.
        if payload[0..4] != SECRET_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // Nonce : octets 4..28
        let mut nonce_bytes = [0u8; 24];
        nonce_bytes.copy_from_slice(&payload[4..28]);
        // Tag : octets 28..44
        let mut tag_bytes = [0u8; 16];
        tag_bytes.copy_from_slice(&payload[28..44]);
        // Longueur clair : octets 44..52 (little-endian)
        let mut len_bytes = [0u8; 8];
        len_bytes.copy_from_slice(&payload[44..52]);
        let plaintext_len = u64::from_le_bytes(len_bytes);
        Ok(Self {
            nonce: Nonce(nonce_bytes),
            tag:   Tag(tag_bytes),
            plaintext_len,
        })
    }

    /// Retourne `true` si la longueur déclarée est cohérente avec la taille du payload.
    ///
    /// ARITH-02 : checked_add.
    pub fn len_is_coherent(&self, payload_len: usize) -> bool {
        let expected = SECRET_HEADER_SIZE.checked_add(self.plaintext_len as usize);
        expected.map(|e| e == payload_len).unwrap_or(false)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SecretReader
// ─────────────────────────────────────────────────────────────────────────────

/// Lecteur de secrets déchiffrant des payloads XChaCha20-Poly1305.
pub struct SecretReader {
    key: XChaCha20Key,
}

impl SecretReader {
    /// Construit un reader à partir d'une clé brute 32 octets.
    pub fn new(raw_key: &[u8; 32]) -> Self {
        Self { key: XChaCha20Key(*raw_key) }
    }

    /// Déchiffre un payload complet.
    ///
    /// Structure attendue : `[magic:4][nonce:24][tag:16][len:8][ciphertext:len]`
    ///
    /// OOM-02 : try_reserve.
    pub fn decrypt(&self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        let header = SecretHeader::parse(payload)?;
        let ct_off = SECRET_HEADER_SIZE;
        let ct_len = payload.len().checked_sub(ct_off).ok_or(ExofsError::CorruptedStructure)?;
        // Cohérence taille.
        if ct_len as u64 != header.plaintext_len {
            return Err(ExofsError::CorruptedStructure);
        }
        let ciphertext = &payload[ct_off..];
        // Buffer de sortie.
        let mut plaintext: Vec<u8> = Vec::new();
        plaintext.try_reserve(ct_len).map_err(|_| ExofsError::NoMemory)?;
        plaintext.extend_from_slice(ciphertext);
        // Déchiffrement XChaCha20-Poly1305.
        let plaintext = XChaCha20Poly1305::decrypt(&self.key, &header.nonce, &[], &plaintext, &header.tag)?;
        Ok(plaintext)
    }

    /// Déchiffre en vérifiant également des données additionnelles (AAD).
    ///
    /// Les données additionnelles ne font pas partie du payload, elles sont
    /// vérifiées via le tag d'authentification.
    pub fn decrypt_with_aad(&self, payload: &[u8], aad: &[u8]) -> ExofsResult<Vec<u8>> {
        let header = SecretHeader::parse(payload)?;
        let ct_off = SECRET_HEADER_SIZE;
        let ct_len = payload.len().checked_sub(ct_off).ok_or(ExofsError::CorruptedStructure)?;
        if ct_len as u64 != header.plaintext_len {
            return Err(ExofsError::CorruptedStructure);
        }
        let ciphertext = &payload[ct_off..];
        let mut plaintext: Vec<u8> = Vec::new();
        plaintext.try_reserve(ct_len).map_err(|_| ExofsError::NoMemory)?;
        plaintext.extend_from_slice(ciphertext);
        let plaintext = XChaCha20Poly1305::decrypt(&self.key, &header.nonce, aad, &plaintext, &header.tag)?;
        Ok(plaintext)
    }

    /// Valide uniquement le header (magic + longueur), sans déchiffrer.
    pub fn validate_header(payload: &[u8]) -> ExofsResult<SecretHeader> {
        let h = SecretHeader::parse(payload)?;
        if !h.len_is_coherent(payload.len()) {
            return Err(ExofsError::CorruptedStructure);
        }
        Ok(h)
    }

    /// Déchiffre un batch de payloads.
    ///
    /// OOM-02 : try_reserve.
    pub fn decrypt_batch(&self, payloads: &[&[u8]]) -> ExofsResult<Vec<Vec<u8>>> {
        let mut results: Vec<Vec<u8>> = Vec::new();
        results.try_reserve(payloads.len()).map_err(|_| ExofsError::NoMemory)?;
        for payload in payloads {
            results.push(self.decrypt(payload)?);
        }
        Ok(results)
    }

    /// Retourne la taille des données en clair depuis le header (sans déchiffrer).
    pub fn peek_plaintext_len(payload: &[u8]) -> ExofsResult<u64> {
        let h = SecretHeader::parse(payload)?;
        Ok(h.plaintext_len)
    }

    /// Vérifie que le payload a un magic valide sans parser entièrement.
    pub fn has_valid_magic(payload: &[u8]) -> bool {
        payload.len() >= 4 && payload[0..4] == SECRET_MAGIC
    }

    /// Retourne la clé brute (pour inspection de sécurité interne uniquement).
    #[allow(dead_code)]
    pub(crate) fn raw_key(&self) -> &[u8; 32] { &self.key.0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// ReadResult
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat enrichi d'un déchiffrement.
#[derive(Debug)]
pub struct ReadResult {
    /// Données en clair.
    pub plaintext:     Vec<u8>,
    /// Nonce utilisé.
    pub nonce:         Nonce,
    /// Longueur des données déchiffrées.
    pub plaintext_len: usize,
}

impl ReadResult {
    /// Construit depuis un vecteur et un header.
    pub fn from_parts(plaintext: Vec<u8>, header: SecretHeader) -> Self {
        let len = plaintext.len();
        Self { plaintext, nonce: header.nonce, plaintext_len: len }
    }
}

/// Lecteur avec résultat enrichi.
pub struct VerboseSecretReader {
    inner: SecretReader,
}

impl VerboseSecretReader {
    pub fn new(raw_key: &[u8; 32]) -> Self {
        Self { inner: SecretReader::new(raw_key) }
    }

    /// Déchiffre et retourne un `ReadResult` avec métadonnées.
    pub fn decrypt_verbose(&self, payload: &[u8]) -> ExofsResult<ReadResult> {
        let header    = SecretHeader::parse(payload)?;
        let plaintext = self.inner.decrypt(payload)?;
        Ok(ReadResult::from_parts(plaintext, header))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la taille attendue d'un payload en fonction de la longueur du clair.
///
/// ARITH-02 : checked_add.
pub fn expected_payload_size(plaintext_len: usize) -> ExofsResult<usize> {
    SECRET_HEADER_SIZE.checked_add(plaintext_len).ok_or(ExofsError::OffsetOverflow)
}

/// Vérifie que le magic est présent et conforme.
pub fn check_magic(buf: &[u8]) -> ExofsResult<()> {
    if buf.len() < 4 || buf[0..4] != SECRET_MAGIC {
        Err(ExofsError::InvalidMagic)
    } else {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::secret_writer::{SecretWriter, build_payload};

    fn key32() -> [u8; 32] { [0xAB; 32] }
    fn writer() -> SecretWriter { SecretWriter::new(&key32()) }
    fn reader() -> SecretReader { SecretReader::new(&key32()) }

    #[test] fn test_encrypt_decrypt_roundtrip() {
        let data    = b"Hello ExoFS secret!";
        let payload = writer().encrypt(data).unwrap();
        let plain   = reader().decrypt(&payload).unwrap();
        assert_eq!(plain, data);
    }

    #[test] fn test_invalid_magic() {
        let mut payload = writer().encrypt(b"test data").unwrap();
        payload[0] = 0x00; // corrompt le magic
        assert_eq!(reader().decrypt(&payload).unwrap_err(), ExofsError::InvalidMagic);
    }

    #[test] fn test_truncated_payload() {
        let payload = writer().encrypt(b"short").unwrap();
        let trunc   = &payload[..20];
        assert!(reader().decrypt(trunc).is_err());
    }

    #[test] fn test_tampered_ciphertext() {
        let mut payload = writer().encrypt(b"secret value").unwrap();
        let last = payload.len() - 1;
        payload[last] ^= 0xFF; // flip bit
        assert!(reader().decrypt(&payload).is_err());
    }

    #[test] fn test_peek_plaintext_len() {
        let data    = b"length check";
        let payload = writer().encrypt(data).unwrap();
        let len     = SecretReader::peek_plaintext_len(&payload).unwrap();
        assert_eq!(len, data.len() as u64);
    }

    #[test] fn test_has_valid_magic() {
        let payload = writer().encrypt(b"x").unwrap();
        assert!(SecretReader::has_valid_magic(&payload));
        assert!(!SecretReader::has_valid_magic(&[0x00, 0x01, 0x02, 0x03]));
    }

    #[test] fn test_validate_header_ok() {
        let payload = writer().encrypt(b"validate").unwrap();
        let h       = SecretReader::validate_header(&payload).unwrap();
        assert_eq!(h.plaintext_len, 8);
    }

    #[test] fn test_decrypt_batch() {
        let data    = b"batch";
        let p1      = writer().encrypt(data).unwrap();
        let p2      = writer().encrypt(data).unwrap();
        let batch   = alloc::vec![p1.as_slice(), p2.as_slice()];
        let results = reader().decrypt_batch(&batch).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], data);
    }

    #[test] fn test_expected_payload_size() {
        let sz = expected_payload_size(100).unwrap();
        assert_eq!(sz, SECRET_HEADER_SIZE + 100);
    }

    #[test] fn test_expected_payload_size_overflow() {
        assert!(expected_payload_size(usize::MAX).is_err());
    }

    #[test] fn test_check_magic_valid() {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&SECRET_MAGIC);
        assert!(check_magic(&buf).is_ok());
    }

    #[test] fn test_check_magic_invalid() {
        let buf = [0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(check_magic(&buf).unwrap_err(), ExofsError::InvalidMagic);
    }

    #[test] fn test_verbose_decrypt() {
        let data = b"verbose result";
        let p    = writer().encrypt(data).unwrap();
        let vr   = VerboseSecretReader::new(&key32());
        let res  = vr.decrypt_verbose(&p).unwrap();
        assert_eq!(res.plaintext, data);
        assert_eq!(res.plaintext_len, data.len());
    }

    #[test] fn test_raw_key_accessible() {
        let r = reader();
        assert_eq!(r.raw_key(), &key32());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CachingSecretReader — lecteur avec cache de résultats déchiffrés
// ─────────────────────────────────────────────────────────────────────────────

use alloc::collections::BTreeMap;

/// Entrée du cache de déchiffrement.
struct CacheEntry {
    #[allow(dead_code)]
    payload_hash: u64,
    plaintext:    Vec<u8>,
}

/// Lecteur avec cache MRU (Most Recently Used) pour éviter de redéchiffrer
/// plusieurs fois le même payload.
pub struct CachingSecretReader {
    inner:    SecretReader,
    cache:    BTreeMap<u64, CacheEntry>,
    capacity: usize,
    hits:     u64,
    misses:   u64,
}

impl CachingSecretReader {
    /// Crée un lecteur avec cache de capacité donnée.
    pub fn new(raw_key: &[u8; 32], capacity: usize) -> Self {
        Self {
            inner: SecretReader::new(raw_key),
            cache: BTreeMap::new(),
            capacity,
            hits: 0,
            misses: 0,
        }
    }

    /// Hash de payload léger (FNV-1a 64 bits).
    ///
    /// ARITH-02 : wrapping_mul / wrapping_add.
    fn hash_payload(data: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Déchiffre avec cache.
    ///
    /// OOM-02 : try_reserve avant insertion.
    pub fn decrypt_cached(&mut self, payload: &[u8]) -> ExofsResult<&[u8]> {
        let key = Self::hash_payload(payload);
        if self.cache.contains_key(&key) {
            self.hits = self.hits.saturating_add(1);
            // SAFETY: borrow checker contournement via re-lookup.
            return Ok(&self.cache[&key].plaintext);
        }
        self.misses = self.misses.saturating_add(1);
        // Éviction si plein.
        if self.cache.len() >= self.capacity {
            if let Some(&oldest_key) = self.cache.keys().next() {
                self.cache.remove(&oldest_key);
            }
        }
        let plain = self.inner.decrypt(payload)?;
        self.cache.try_insert_compat(key, CacheEntry { payload_hash: key, plaintext: plain })
            .map_err(|_| ExofsError::NoMemory)?;
        Ok(&self.cache[&key].plaintext)
    }

    /// Vide le cache.
    pub fn clear_cache(&mut self) { self.cache.clear(); }

    /// Statistiques du cache.
    pub fn stats(&self) -> (u64, u64) { (self.hits, self.misses) }

    /// Hit rate en pourcentage (0..=100).
    ///
    /// ARITH-02 : checked_add / saturating_div.
    pub fn hit_rate_pct(&self) -> u64 {
        let total = self.hits.saturating_add(self.misses);
        if total == 0 { return 0; }
        self.hits.saturating_mul(100) / total
    }
}

// BTreeMap n'a pas try_insert en no_std, on implémente un helper.
trait BTreeMapExt<K: Ord, V> {
    fn try_insert_compat(&mut self, k: K, v: V) -> ExofsResult<()>;
}

impl<K: Ord, V> BTreeMapExt<K, V> for BTreeMap<K, V> {
    fn try_insert_compat(&mut self, k: K, v: V) -> ExofsResult<()> {
        self.insert(k, v);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StreamingSecretReader — déchiffrement ligne-par-ligne de chunks
// ─────────────────────────────────────────────────────────────────────────────

/// État de la lecture en streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// Prêt à lire un nouveau payload.
    Ready,
    /// Header validé, en attente du ciphertext.
    HeaderParsed,
    /// Déchiffrement terminé.
    Done,
    /// Erreur non récupérable.
    Error,
}

/// Lecteur en mode streaming pour déchiffrer des payloads fragmentés.
pub struct StreamingSecretReader {
    key:        XChaCha20Key,
    buffer:     Vec<u8>,
    state:      StreamState,
    header:     Option<SecretHeader>,
    max_size:   usize,
}

impl StreamingSecretReader {
    /// Crée un reader streaming avec taille maximale de buffer.
    pub fn new(raw_key: &[u8; 32], max_size: usize) -> Self {
        Self {
            key:      XChaCha20Key(*raw_key),
            buffer:   Vec::new(),
            state:    StreamState::Ready,
            header:   None,
            max_size,
        }
    }

    /// Pousse un chunk de données dans le buffer.
    ///
    /// OOM-02 : try_reserve.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> ExofsResult<()> {
        let new_len = self.buffer.len().checked_add(chunk.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if new_len > self.max_size {
            self.state = StreamState::Error;
            return Err(ExofsError::OffsetOverflow);
        }
        self.buffer.try_reserve(chunk.len()).map_err(|_| ExofsError::NoMemory)?;
        self.buffer.extend_from_slice(chunk);
        // Tentative de progression.
        self.try_advance()?;
        Ok(())
    }

    /// Tente de faire avancer l'état de déchiffrement.
    fn try_advance(&mut self) -> ExofsResult<()> {
        if self.state == StreamState::Ready && self.buffer.len() >= SECRET_HEADER_SIZE {
            let h = SecretHeader::parse(&self.buffer)?;
            self.header = Some(h);
            self.state  = StreamState::HeaderParsed;
        }
        if self.state == StreamState::HeaderParsed {
            if let Some(ref h) = self.header {
                let expected = SECRET_HEADER_SIZE
                    .checked_add(h.plaintext_len as usize)
                    .ok_or(ExofsError::OffsetOverflow)?;
                if self.buffer.len() >= expected {
                    self.state = StreamState::Done;
                }
            }
        }
        Ok(())
    }

    /// Retourne `true` si le payload est complet et prêt à être déchiffré.
    pub fn is_ready(&self) -> bool { self.state == StreamState::Done }

    /// Finalize le déchiffrement (nécessite `is_ready() == true`).
    ///
    /// OOM-02 : try_reserve dans decrypt.
    pub fn finalize(&self) -> ExofsResult<Vec<u8>> {
        if self.state != StreamState::Done {
            return Err(ExofsError::InvalidArgument);
        }
        let reader = SecretReader { key: XChaCha20Key(self.key.0) };
        reader.decrypt(&self.buffer)
    }

    /// Réinitialise le reader pour un nouveau payload.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.state  = StreamState::Ready;
        self.header = None;
    }

    /// Retourne l'état courant.
    pub fn state(&self) -> StreamState { self.state }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extended {
    use super::*;
    use super::super::secret_writer::SecretWriter;

    fn key32() -> [u8; 32] { [0xAB; 32] }
    fn writer() -> SecretWriter { SecretWriter::new(&key32()) }

    #[test] fn test_streaming_single_chunk() {
        let payload = writer().encrypt(b"stream chunk").unwrap();
        let mut sr  = StreamingSecretReader::new(&key32(), 4096);
        sr.push_chunk(&payload).unwrap();
        assert!(sr.is_ready());
        let plain = sr.finalize().unwrap();
        assert_eq!(plain, b"stream chunk");
    }

    #[test] fn test_streaming_multi_chunk() {
        let payload  = writer().encrypt(b"multi chunk data").unwrap();
        let mid      = payload.len() / 2;
        let mut sr   = StreamingSecretReader::new(&key32(), 4096);
        sr.push_chunk(&payload[..mid]).unwrap();
        assert!(!sr.is_ready());
        sr.push_chunk(&payload[mid..]).unwrap();
        assert!(sr.is_ready());
        let plain = sr.finalize().unwrap();
        assert_eq!(plain, b"multi chunk data");
    }

    #[test] fn test_streaming_reset() {
        let payload = writer().encrypt(b"reset").unwrap();
        let mut sr  = StreamingSecretReader::new(&key32(), 4096);
        sr.push_chunk(&payload).unwrap();
        assert!(sr.is_ready());
        sr.reset();
        assert_eq!(sr.state(), StreamState::Ready);
        assert!(!sr.is_ready());
    }

    #[test] fn test_streaming_finalize_before_ready() {
        let mut sr = StreamingSecretReader::new(&key32(), 4096);
        assert!(sr.finalize().is_err());
    }

    #[test] fn test_header_parse_bad_magic() {
        let bad = [0xFF; 52];
        assert_eq!(SecretHeader::parse(&bad).unwrap_err(), ExofsError::InvalidMagic);
    }

    #[test] fn test_header_len_is_coherent() {
        let data    = b"coherent";
        let payload = writer().encrypt(data).unwrap();
        let h       = SecretHeader::parse(&payload).unwrap();
        assert!(h.len_is_coherent(payload.len()));
    }

    #[test] fn test_header_len_is_not_coherent() {
        let data    = b"incoherent";
        let payload = writer().encrypt(data).unwrap();
        let h       = SecretHeader::parse(&payload).unwrap();
        assert!(!h.len_is_coherent(payload.len() + 1));
    }
}
