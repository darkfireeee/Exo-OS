//! Écrivain de secrets chiffrés — chiffrement de blobs XChaCha20-Poly1305.
//!
//! FORMAT DU PAYLOAD GÉNÉRÉ :
//!   [magic: 4 octets][nonce: 24 octets][tag: 16 octets][longueur_clair: 8 octets][ciphertext: N octets]
//!   → Header total = 52 octets.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

#![allow(dead_code)]

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::xchacha20::{XChaCha20Key, XChaCha20Poly1305, Nonce, Tag};
use super::entropy::ENTROPY_POOL;
use super::secret_reader::SECRET_MAGIC;

pub use super::secret_reader::SECRET_HEADER_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une écriture chiffrée.
#[derive(Debug)]
pub struct SecretWriteResult {
    /// Payload prêt à persister.
    pub payload:   Vec<u8>,
    /// Nonce utilisé.
    pub nonce:     Nonce,
    /// Tag d'authentification.
    pub tag:       Tag,
    /// Longueur du plaintext original.
    pub plain_len: usize,
}

/// Blob chiffré avec accès structuré aux composants.
#[derive(Debug, Clone)]
pub struct EncryptedBlob {
    /// Payload complet (header + ciphertext).
    pub raw:       Vec<u8>,
    /// Identifiant interne (assigné par l'appelant).
    pub blob_id:   u64,
    /// Longueur des données en clair.
    pub plain_len: u64,
}

impl EncryptedBlob {
    /// Retourne une référence sur les bytes du payload.
    pub fn as_bytes(&self) -> &[u8] { &self.raw }

    /// Taille totale du payload.
    pub fn payload_len(&self) -> usize { self.raw.len() }

    /// Retourne la taille du ciphertext (sans header).
    ///
    /// ARITH-02 : saturating_sub.
    pub fn ciphertext_len(&self) -> usize { self.raw.len().saturating_sub(SECRET_HEADER_SIZE) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constructeur de payload — fonction publique utilitaire
// ─────────────────────────────────────────────────────────────────────────────

/// Construit un payload chiffré à partir d'une clé, d'un nonce, d'un tag et
/// du ciphertext.
///
/// OOM-02 : try_reserve.
/// ARITH-02 : checked_add.
pub fn build_payload(nonce: &Nonce, tag: &Tag, ciphertext: &[u8]) -> ExofsResult<Vec<u8>> {
    let total = SECRET_HEADER_SIZE
        .checked_add(ciphertext.len())
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut payload: Vec<u8> = Vec::new();
    payload.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    // Magic
    payload.extend_from_slice(&SECRET_MAGIC);
    // Nonce (24)
    payload.extend_from_slice(&nonce.0);
    // Tag (16)
    payload.extend_from_slice(&tag.0);
    // Longueur clair (8 LE)
    payload.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    // Ciphertext
    payload.extend_from_slice(ciphertext);
    Ok(payload)
}

// ─────────────────────────────────────────────────────────────────────────────
// SecretWriter
// ─────────────────────────────────────────────────────────────────────────────

/// Écrivain de secrets chiffrant des données claires vers le format ExoFS.
pub struct SecretWriter {
    key: XChaCha20Key,
}

impl SecretWriter {
    /// Construit un writer à partir d'une clé brute 32 octets.
    pub fn new(raw_key: &[u8; 32]) -> Self {
        Self { key: XChaCha20Key(*raw_key) }
    }

    /// Chiffre des données et retourne le payload prêt à persister.
    ///
    /// Génère un nonce aléatoire via `ENTROPY_POOL`.  
    /// OOM-02 : try_reserve.
    pub fn encrypt(&self, plaintext: &[u8]) -> ExofsResult<Vec<u8>> {
        if plaintext.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }
        let nonce_bytes = ENTROPY_POOL.random_nonce_24();
        let nonce       = Nonce(nonce_bytes);
        // Buffer de travail (copie du plaintext pour chiffrement en place).
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(plaintext.len()).map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(plaintext);
        let tag = XChaCha20Poly1305::encrypt(&self.key, &nonce, &mut buf)?;
        build_payload(&nonce, &tag, &buf)
    }

    /// Chiffre avec données additionnelles authentifiées (AAD).
    ///
    /// Les AAD sont intégrées dans le tag mais pas dans le payload.
    pub fn encrypt_with_aad(&self, plaintext: &[u8], aad: &[u8]) -> ExofsResult<Vec<u8>> {
        if plaintext.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }
        let nonce_bytes = ENTROPY_POOL.random_nonce_24();
        let nonce       = Nonce(nonce_bytes);
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(plaintext.len()).map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(plaintext);
        let tag = XChaCha20Poly1305::encrypt_aad(&self.key, &nonce, &mut buf, aad)?;
        build_payload(&nonce, &tag, &buf)
    }

    /// Chiffre et retourne un `SecretWriteResult` avec métadonnées.
    pub fn encrypt_verbose(&self, plaintext: &[u8]) -> ExofsResult<SecretWriteResult> {
        if plaintext.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }
        let nonce_bytes = ENTROPY_POOL.random_nonce_24();
        let nonce       = Nonce(nonce_bytes);
        let plain_len   = plaintext.len();
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(plain_len).map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(plaintext);
        let tag     = XChaCha20Poly1305::encrypt(&self.key, &nonce, &mut buf)?;
        let payload = build_payload(&nonce, &tag, &buf)?;
        Ok(SecretWriteResult { payload, nonce, tag, plain_len })
    }

    /// Chiffre un vecteur de données et construit un `EncryptedBlob`.
    pub fn encrypt_as_blob(&self, plaintext: &[u8], blob_id: u64) -> ExofsResult<EncryptedBlob> {
        let plain_len = plaintext.len() as u64;
        let raw       = self.encrypt(plaintext)?;
        Ok(EncryptedBlob { raw, blob_id, plain_len })
    }

    /// Chiffre un batch de données.
    ///
    /// OOM-02 : try_reserve.
    pub fn encrypt_batch(&self, items: &[&[u8]]) -> ExofsResult<Vec<Vec<u8>>> {
        let mut results: Vec<Vec<u8>> = Vec::new();
        results.try_reserve(items.len()).map_err(|_| ExofsError::NoMemory)?;
        for &item in items {
            results.push(self.encrypt(item)?);
        }
        Ok(results)
    }

    /// Retourne la clé brute (pour audit de sécurité interne uniquement).
    pub(crate) fn raw_key(&self) -> &[u8; 32] { &self.key.0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// SecretWriterPool — pool de writers réutilisables
// ─────────────────────────────────────────────────────────────────────────────

use alloc::collections::BTreeMap;

/// Identifiant d'un writer dans le pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WriterId(pub u64);

/// Pool de writers indéxés par ID.
pub struct SecretWriterPool {
    writers:  BTreeMap<WriterId, SecretWriter>,
    next_id:  u64,
    capacity: usize,
}

impl SecretWriterPool {
    /// Crée un pool de capacité maximale.
    pub fn new(capacity: usize) -> Self {
        Self { writers: BTreeMap::new(), next_id: 1, capacity }
    }

    /// Ajoute un writer et retourne son ID.
    ///
    /// OOM-02 : vérification de capacité.
    pub fn add_writer(&mut self, raw_key: &[u8; 32]) -> ExofsResult<WriterId> {
        if self.writers.len() >= self.capacity {
            return Err(ExofsError::NoMemory);
        }
        let id = WriterId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.writers.insert(id, SecretWriter::new(raw_key));
        Ok(id)
    }

    /// Retourne une référence vers un writer.
    pub fn get(&self, id: WriterId) -> ExofsResult<&SecretWriter> {
        self.writers.get(&id).ok_or(ExofsError::ObjectNotFound)
    }

    /// Chiffre via un writer du pool.
    pub fn encrypt_with(&self, id: WriterId, plain: &[u8]) -> ExofsResult<Vec<u8>> {
        self.get(id)?.encrypt(plain)
    }

    /// Supprime un writer du pool.
    pub fn remove(&mut self, id: WriterId) { self.writers.remove(&id); }

    /// Nombre de writers actifs.
    pub fn len(&self) -> usize { self.writers.len() }

    /// Retourne `true` si le pool est vide.
    pub fn is_empty(&self) -> bool { self.writers.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule la taille d'un payload chiffré à partir de la taille des données claires.
///
/// ARITH-02 : checked_add.
pub fn payload_size_for(plaintext_len: usize) -> ExofsResult<usize> {
    SECRET_HEADER_SIZE.checked_add(plaintext_len).ok_or(ExofsError::OffsetOverflow)
}

/// Vérifie qu'une taille de payload est valide (header + au moins 1 octet).
pub fn is_valid_payload_size(size: usize) -> bool {
    size > SECRET_HEADER_SIZE
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::secret_reader::SecretReader;

    fn key32() -> [u8; 32] { [0xAB; 32] }
    fn writer() -> SecretWriter { SecretWriter::new(&key32()) }
    fn reader() -> SecretReader { SecretReader::new(&key32()) }

    #[test] fn test_encrypt_produces_payload() {
        let p = writer().encrypt(b"some data").unwrap();
        assert!(p.len() > SECRET_HEADER_SIZE);
    }

    #[test] fn test_encrypt_empty_data_fails() {
        assert_eq!(writer().encrypt(b"").unwrap_err(), ExofsError::InvalidArgument);
    }

    #[test] fn test_payload_starts_with_magic() {
        use super::super::secret_reader::SECRET_MAGIC;
        let p = writer().encrypt(b"magic check").unwrap();
        assert_eq!(&p[0..4], &SECRET_MAGIC);
    }

    #[test] fn test_encrypt_decrypt_roundtrip() {
        let data    = b"roundtrip data";
        let payload = writer().encrypt(data).unwrap();
        let plain   = reader().decrypt(&payload).unwrap();
        assert_eq!(plain, data);
    }

    #[test] fn test_encrypt_verbose_fields() {
        let res = writer().encrypt_verbose(b"verbose").unwrap();
        assert_eq!(res.plain_len, 7);
        assert!(!res.payload.is_empty());
    }

    #[test] fn test_encrypt_as_blob() {
        let blob = writer().encrypt_as_blob(b"blob data", 42).unwrap();
        assert_eq!(blob.blob_id, 42);
        assert_eq!(blob.plain_len, 9);
        assert_eq!(blob.ciphertext_len(), 9);
    }

    #[test] fn test_build_payload_structure() {
        use super::super::secret_reader::SECRET_MAGIC;
        let nonce = Nonce([0u8; 24]);
        let tag   = Tag([0u8; 16]);
        let ct    = &[0xAA; 10];
        let p     = build_payload(&nonce, &tag, ct).unwrap();
        assert_eq!(p.len(), SECRET_HEADER_SIZE + 10);
        assert_eq!(&p[0..4], &SECRET_MAGIC);
    }

    #[test] fn test_payload_size_for_ok() {
        let sz = payload_size_for(100).unwrap();
        assert_eq!(sz, SECRET_HEADER_SIZE + 100);
    }

    #[test] fn test_payload_size_for_overflow() {
        assert!(payload_size_for(usize::MAX).is_err());
    }

    #[test] fn test_is_valid_payload_size() {
        assert!(!is_valid_payload_size(SECRET_HEADER_SIZE));
        assert!(is_valid_payload_size(SECRET_HEADER_SIZE + 1));
    }

    #[test] fn test_batch_encrypt() {
        let items: &[&[u8]] = &[b"one", b"two", b"three"];
        let results = writer().encrypt_batch(items).unwrap();
        assert_eq!(results.len(), 3);
        for (i, r) in results.iter().enumerate() {
            let plain = reader().decrypt(r).unwrap();
            assert_eq!(plain, items[i]);
        }
    }

    #[test] fn test_pool_add_and_encrypt() {
        let mut pool = SecretWriterPool::new(4);
        let id       = pool.add_writer(&key32()).unwrap();
        let payload  = pool.encrypt_with(id, b"pool test").unwrap();
        let plain    = reader().decrypt(&payload).unwrap();
        assert_eq!(plain, b"pool test");
    }

    #[test] fn test_pool_capacity_limit() {
        let mut pool = SecretWriterPool::new(1);
        pool.add_writer(&key32()).unwrap();
        assert!(pool.add_writer(&key32()).is_err());
    }

    #[test] fn test_pool_remove() {
        let mut pool = SecretWriterPool::new(4);
        let id       = pool.add_writer(&key32()).unwrap();
        assert_eq!(pool.len(), 1);
        pool.remove(id);
        assert_eq!(pool.len(), 0);
    }

    #[test] fn test_pool_get_missing() {
        let pool = SecretWriterPool::new(4);
        assert!(pool.get(WriterId(99)).is_err());
    }

    #[test] fn test_different_plaintexts_different_pay() {
        let p1 = writer().encrypt(b"aaa").unwrap();
        let p2 = writer().encrypt(b"bbb").unwrap();
        // Les payloads sont différents (données différentes).
        assert_ne!(p1[SECRET_HEADER_SIZE..], p2[SECRET_HEADER_SIZE..]);
    }

    #[test] fn test_nonce_freshness() {
        // Deux chiffrements du même plaintext génèrent des nonces différents.
        let p1 = writer().encrypt(b"same").unwrap();
        let p2 = writer().encrypt(b"same").unwrap();
        assert_ne!(&p1[4..28], &p2[4..28]);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SealedEnvelope — enveloppe chiffrée avec métadonnées
// ─────────────────────────────────────────────────────────────────────────────

/// Enveloppe chiffrée représentant une unité atomique de données secrètes.
#[derive(Debug, Clone)]
pub struct SealedEnvelope {
    /// Identifiant unique de l'enveloppe (assigné par le créateur).
    pub envelope_id:  u64,
    /// Étiquette applicative (ex : "user.password", "token.api").
    pub label:        [u8; 32],
    /// Payload chiffré (header ExoFS + ciphertext).
    pub payload:      Vec<u8>,
    /// Taille des données en clair (pour allocation future côté reader).
    pub plain_len:    u64,
    /// Tick de création (fourni par l'appelant).
    pub created_tick: u64,
}

impl SealedEnvelope {
    /// Retourne la taille complète du payload.
    pub fn payload_len(&self) -> usize { self.payload.len() }

    /// Retourne l'étiquette sous forme de slice jusqu'au premier 0.
    pub fn label_str(&self) -> &[u8] {
        let end = self.label.iter().position(|&b| b == 0).unwrap_or(32);
        &self.label[..end]
    }
}

/// Constructeur d'enveloppes.
pub struct EnvelopeWriter {
    writer:   SecretWriter,
    next_env: u64,
}

impl EnvelopeWriter {
    /// Crée un constructeur avec la clé donnée.
    pub fn new(raw_key: &[u8; 32]) -> Self {
        Self { writer: SecretWriter::new(raw_key), next_env: 1 }
    }

    /// Scelle des données dans une enveloppe.
    ///
    /// `label` doit tenir en 32 octets (tronqué sinon).
    /// OOM-02 : délégué à `encrypt`.
    pub fn seal(
        &mut self,
        plaintext:    &[u8],
        label:        &[u8],
        created_tick: u64,
    ) -> ExofsResult<SealedEnvelope> {
        let payload    = self.writer.encrypt(plaintext)?;
        let plain_len  = plaintext.len() as u64;
        let envelope_id = self.next_env;
        self.next_env = self.next_env.saturating_add(1);
        let mut label_buf = [0u8; 32];
        let copy_len = label.len().min(32);
        label_buf[..copy_len].copy_from_slice(&label[..copy_len]);
        Ok(SealedEnvelope { envelope_id, label: label_buf, payload, plain_len, created_tick })
    }

    /// Scelle un batch d'enveloppes.
    ///
    /// OOM-02 : try_reserve.
    pub fn seal_batch(
        &mut self,
        items:        &[(&[u8], &[u8])],  // (plaintext, label)
        created_tick: u64,
    ) -> ExofsResult<Vec<SealedEnvelope>> {
        let mut results: Vec<SealedEnvelope> = Vec::new();
        results.try_reserve(items.len()).map_err(|_| ExofsError::NoMemory)?;
        for &(pt, lbl) in items {
            results.push(self.seal(pt, lbl, created_tick)?);
        }
        Ok(results)
    }

    /// Identifiant de la prochaine enveloppe.
    pub fn next_envelope_id(&self) -> u64 { self.next_env }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_envelope {
    use super::*;
    use super::super::secret_reader::SecretReader;

    fn key32() -> [u8; 32] { [0xCD; 32] }
    fn reader() -> SecretReader { SecretReader::new(&key32()) }

    #[test] fn test_seal_ok() {
        let mut w = EnvelopeWriter::new(&key32());
        let env   = w.seal(b"secret data", b"test.label", 100).unwrap();
        assert_eq!(env.envelope_id, 1);
        assert_eq!(env.plain_len, 11);
        assert!(!env.payload.is_empty());
    }

    #[test] fn test_seal_decrypt() {
        let mut w   = EnvelopeWriter::new(&key32());
        let env     = w.seal(b"my secret", b"my.key", 0).unwrap();
        let plain   = reader().decrypt(&env.payload).unwrap();
        assert_eq!(plain, b"my secret");
    }

    #[test] fn test_seal_label_truncation() {
        let mut w     = EnvelopeWriter::new(&key32());
        let long_lbl  = &[b'X'; 64];
        let env = w.seal(b"data", long_lbl, 0).unwrap();
        assert_eq!(env.label_str().len(), 32);
    }

    #[test] fn test_seal_increments_id() {
        let mut w = EnvelopeWriter::new(&key32());
        let e1 = w.seal(b"first", b"k1", 0).unwrap();
        let e2 = w.seal(b"second", b"k2", 0).unwrap();
        assert_eq!(e1.envelope_id, 1);
        assert_eq!(e2.envelope_id, 2);
    }

    #[test] fn test_seal_batch_ok() {
        let mut w   = EnvelopeWriter::new(&key32());
        let items   = [(b"aa" as &[u8], b"l1" as &[u8]), (b"bb", b"l2")];
        let results = w.seal_batch(&items, 0).unwrap();
        assert_eq!(results.len(), 2);
        let p0 = reader().decrypt(&results[0].payload).unwrap();
        assert_eq!(p0, b"aa");
    }

    #[test] fn test_payload_len_ok() {
        let mut w  = EnvelopeWriter::new(&key32());
        let env    = w.seal(b"x", b"k", 0).unwrap();
        assert_eq!(env.payload_len(), SECRET_HEADER_SIZE + 1);
    }

    #[test] fn test_label_str_no_null() {
        let mut w   = EnvelopeWriter::new(&key32());
        let env     = w.seal(b"data", b"hello", 0).unwrap();
        assert_eq!(env.label_str(), b"hello");
    }

    #[test] fn test_seal_empty_plaintext_fails() {
        let mut w = EnvelopeWriter::new(&key32());
        assert!(w.seal(b"", b"k", 0).is_err());
    }
}
