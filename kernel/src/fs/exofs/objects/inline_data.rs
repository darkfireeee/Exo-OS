// SPDX-License-Identifier: MIT
// ExoFS — inline_data.rs
// Données embarquées directement dans le LogicalObject (< INLINE_DATA_MAX octets).
// Règles :
//   ARITH-02  : checked_add / saturating_* pour tout calcul d'offset
//   ONDISK-01 : InlineDataDisk → #[repr(C, packed)], types plain uniquement
//   HASH-01   : BlobId calculé sur données brutes (méthode seal())

use crate::fs::exofs::core::{blake3_hash, BlobId, ExofsError, ExofsResult, INLINE_DATA_MAX};
use crate::fs::exofs::objects::object_meta::crc32_compute;
use core::fmt;
use core::mem;

// ── Constantes ─────────────────────────────────────────────────────────────────

/// Taille maximale des données inline (512 octets).
const INLINE_BUF_SIZE: usize = INLINE_DATA_MAX; // 512

/// Taille on-disk d'`InlineDataDisk`.
pub const INLINE_DATA_DISK_SIZE: usize = mem::size_of::<InlineDataDisk>();

// ── Représentation on-disk ─────────────────────────────────────────────────────

/// Données inline persistées sur disque.
///
/// Règle ONDISK-01 : `#[repr(C, packed)]`, types plain uniquement.
///
/// Layout (568 octets) :
/// ```text
///   0..  1   len          u16  — longueur réelle des données
///   2..  3   _pad0        [u8;2]
///   4..  7   checksum     u32  — CRC32 du buffer [buf, len]
///   8..519   buf          [u8;512]
/// 520..551   content_hash [u8;32] — Blake3 du contenu brut (HASH-01)
/// 552..567   _pad1        [u8;16]
/// ```
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct InlineDataDisk {
    /// Longueur réelle des données (≤ 512).
    pub len: u16,
    pub _pad0: [u8; 2],
    /// CRC32 du buffer.
    pub checksum: u32,
    /// Données inline (zéro-padées après `len` octets).
    pub buf: [u8; INLINE_BUF_SIZE],
    /// Hash Blake3 du contenu brut (HASH-01, calculé avant compression).
    pub content_hash: [u8; 32],
    pub _pad1: [u8; 16],
}

// Vérification de taille en compile-time.
const _: () = assert!(
    mem::size_of::<InlineDataDisk>() == 568,
    "InlineDataDisk doit faire 568 octets (ONDISK-01)"
);

// ── InlineData in-memory ───────────────────────────────────────────────────────

/// Données inlinées dans un `LogicalObject` (< 512 octets).
///
/// Quand la taille d'un objet est inférieure à `INLINE_DATA_MAX`,
/// ses données sont stockées directement dans ce tampon plutôt que
/// dans un P-Blob externe. Cela évite toute allocation disque pour
/// les petits objets (configurations, identifiants, …).
#[derive(Clone)]
pub struct InlineData {
    /// Buffer de données brutes.
    buf: [u8; INLINE_BUF_SIZE],
    /// Longueur réelle du contenu (≤ INLINE_BUF_SIZE).
    len: usize,
    /// Hash Blake3 du contenu (HASH-01 : calculé avant compression).
    content_hash: Option<BlobId>,
    /// Indique si le hash est à jour par rapport au buffer.
    hash_valid: bool,
}

impl InlineData {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Crée un `InlineData` vide.
    pub const fn empty() -> Self {
        Self {
            buf: [0u8; INLINE_BUF_SIZE],
            len: 0,
            content_hash: None,
            hash_valid: false,
        }
    }

    /// Crée un `InlineData` depuis une slice.
    ///
    /// Retourne `ExofsError::InlineTooLarge` si `data.len() > INLINE_DATA_MAX`.
    pub fn from_slice(data: &[u8]) -> ExofsResult<Self> {
        if data.len() > INLINE_DATA_MAX {
            return Err(ExofsError::InlineTooLarge);
        }
        let mut buf = [0u8; INLINE_BUF_SIZE];
        buf[..data.len()].copy_from_slice(data);
        Ok(Self {
            buf,
            len: data.len(),
            content_hash: None,
            hash_valid: false,
        })
    }

    /// Reconstruit depuis la représentation disque après vérification CRC32.
    pub fn from_disk(d: &InlineDataDisk) -> ExofsResult<Self> {
        // Vérification CRC32 du buffer (HDR-03 analogue pour inline).
        let stored = d.checksum;
        let computed = inline_crc32(d);
        if stored != computed {
            return Err(ExofsError::Corrupt);
        }
        let len = d.len as usize;
        if len > INLINE_BUF_SIZE {
            return Err(ExofsError::Corrupt);
        }

        let hash = BlobId(d.content_hash);
        Ok(Self {
            buf: d.buf,
            len,
            content_hash: Some(hash),
            hash_valid: true,
        })
    }

    // ── Sérialisation ─────────────────────────────────────────────────────────

    /// Sérialise vers la représentation disque.
    ///
    /// Calcule automatiquement le hash Blake3 si `hash_valid` est faux
    /// (règle HASH-01 : le hash est calculé sur les données brutes).
    pub fn to_disk(&mut self) -> InlineDataDisk {
        if !self.hash_valid {
            self.compute_hash();
        }
        let content_hash = self.content_hash.as_ref().map(|b| b.0).unwrap_or([0u8; 32]);

        let mut d = InlineDataDisk {
            len: self.len as u16,
            _pad0: [0u8; 2],
            checksum: 0,
            buf: self.buf,
            content_hash,
            _pad1: [0u8; 16],
        };
        d.checksum = inline_crc32(&d);
        d
    }

    // ── Accès lectures ────────────────────────────────────────────────────────

    /// Retourne la tranche des données réelles (sans le padding nul).
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Retourne la longueur réelle du contenu.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Vrai si les données inline sont vides (longueur 0).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Retourne la capacité maximale (= `INLINE_DATA_MAX`).
    #[inline]
    pub fn capacity() -> usize {
        INLINE_BUF_SIZE
    }

    // ── Hash Blake3 (HASH-01) ─────────────────────────────────────────────────

    /// Calcule et met en cache le hash Blake3 du contenu brut.
    ///
    /// Règle HASH-01 : calculé sur les données **avant** compression.
    pub fn compute_hash(&mut self) -> BlobId {
        let hash = blake3_hash(&self.buf[..self.len]);
        let id = BlobId(hash);
        self.content_hash = Some(id);
        self.hash_valid = true;
        id
    }

    /// Retourne le hash du contenu, en le calculant si nécessaire.
    pub fn get_or_compute_hash(&mut self) -> BlobId {
        if !self.hash_valid {
            self.compute_hash()
        } else {
            self.content_hash.unwrap_or_else(|| self.compute_hash())
        }
    }

    /// Vérifie que le hash stocké correspond au contenu courant.
    pub fn verify_hash(&self) -> bool {
        match &self.content_hash {
            Some(stored) => {
                let computed = blake3_hash(&self.buf[..self.len]);
                stored.0 == computed
            }
            None => false,
        }
    }

    // ── Modifications ─────────────────────────────────────────────────────────

    /// Remplace intégralement le contenu inline.
    ///
    /// Invalide le hash (sera recalculé au prochain `to_disk()` ou `compute_hash()`).
    pub fn update(&mut self, data: &[u8]) -> ExofsResult<()> {
        if data.len() > INLINE_BUF_SIZE {
            return Err(ExofsError::InlineTooLarge);
        }
        // Efface d'abord l'ancienne zone pour ne pas laisser de données résiduelles.
        self.buf[..self.len].fill(0);
        self.buf[..data.len()].copy_from_slice(data);
        self.len = data.len();
        self.hash_valid = false;
        self.content_hash = None;
        Ok(())
    }

    /// Écrit `src` à l'offset logique `offset` à l'intérieur du buffer inline.
    ///
    /// Étend automatiquement `len` si nécessaire. Retourne une erreur si
    /// `offset + src.len()` dépasserait `INLINE_BUF_SIZE`.
    pub fn write_at(&mut self, offset: usize, src: &[u8]) -> ExofsResult<()> {
        let end = offset.checked_add(src.len()).ok_or(ExofsError::Overflow)?;
        if end > INLINE_BUF_SIZE {
            return Err(ExofsError::InlineTooLarge);
        }
        self.buf[offset..end].copy_from_slice(src);
        if end > self.len {
            self.len = end;
        }
        self.hash_valid = false;
        self.content_hash = None;
        Ok(())
    }

    /// Lit `len` octets depuis l'offset logique `offset` vers `dst`.
    ///
    /// Retourne `ExofsError::OutOfRange` si la plage dépasse le contenu.
    pub fn read_at(&self, offset: usize, dst: &mut [u8]) -> ExofsResult<usize> {
        if offset >= self.len {
            return Ok(0); // Lecture au-delà de la fin : retourne 0 octet lu.
        }
        let available = self.len - offset;
        let to_copy = dst.len().min(available);
        dst[..to_copy].copy_from_slice(&self.buf[offset..offset + to_copy]);
        Ok(to_copy)
    }

    /// Ajoute des données à la fin du buffer inline.
    ///
    /// Retourne `ExofsError::InlineTooLarge` si la capacité serait dépassée.
    pub fn append(&mut self, data: &[u8]) -> ExofsResult<()> {
        let new_len = self
            .len
            .checked_add(data.len())
            .ok_or(ExofsError::Overflow)?;
        if new_len > INLINE_BUF_SIZE {
            return Err(ExofsError::InlineTooLarge);
        }
        self.buf[self.len..new_len].copy_from_slice(data);
        self.len = new_len;
        self.hash_valid = false;
        self.content_hash = None;
        Ok(())
    }

    /// Tronque le contenu à `new_len` octets.
    ///
    /// Les octets supprimés sont écrasés par des zéros.
    pub fn truncate(&mut self, new_len: usize) -> ExofsResult<()> {
        if new_len > self.len {
            return Err(ExofsError::InvalidArgument);
        }
        self.buf[new_len..self.len].fill(0);
        self.len = new_len;
        self.hash_valid = false;
        self.content_hash = None;
        Ok(())
    }

    /// Copie tout le contenu vers `dst`.
    ///
    /// Retourne le nombre d'octets copiés.
    pub fn copy_to(&self, dst: &mut [u8]) -> usize {
        let n = dst.len().min(self.len);
        dst[..n].copy_from_slice(&self.buf[..n]);
        n
    }

    /// Compare le contenu avec une slice externe (comparaison en temps constant
    /// pour éviter les timing attacks).
    pub fn ct_eq(&self, other: &[u8]) -> bool {
        if self.len != other.len() {
            return false;
        }
        let mut diff: u8 = 0;
        for (&a, &b) in self.buf[..self.len].iter().zip(other.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// Effacement sécurisé du buffer (zéro-remplissage).
    pub fn zeroize(&mut self) {
        self.buf.fill(0);
        self.len = 0;
        self.hash_valid = false;
        self.content_hash = None;
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /// Valide la cohérence interne de l'`InlineData`.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.len > INLINE_BUF_SIZE {
            return Err(ExofsError::Corrupt);
        }
        // Les octets au-delà de `len` doivent être zéro.
        for &b in &self.buf[self.len..] {
            if b != 0 {
                return Err(ExofsError::Corrupt);
            }
        }
        Ok(())
    }
}

// ── Display / Debug ────────────────────────────────────────────────────────────

impl fmt::Display for InlineData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InlineData {{ len: {}/{}, hash_valid: {} }}",
            self.len, INLINE_BUF_SIZE, self.hash_valid,
        )
    }
}

impl fmt::Debug for InlineData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── InlineDataStats ────────────────────────────────────────────────────────────

/// Statistiques sur les opérations sur données inline.
#[derive(Default, Debug, Clone)]
pub struct InlineDataStats {
    /// Nombre de sérialisations vers disque.
    pub to_disk_count: u64,
    /// Nombre de lectures depuis disque.
    pub from_disk_count: u64,
    /// Nombre d'erreurs CRC32.
    pub checksum_errors: u64,
    /// Nombre de calculs de hash Blake3.
    pub hash_computations: u64,
    /// Nombre d'opérations write_at.
    pub write_at_count: u64,
    /// Nombre d'opérations read_at.
    pub read_at_count: u64,
    /// Nombre d'appels append.
    pub append_count: u64,
    /// Nombre d'appels truncate.
    pub truncate_count: u64,
}

impl InlineDataStats {
    pub const fn new() -> Self {
        Self {
            to_disk_count: 0,
            from_disk_count: 0,
            checksum_errors: 0,
            hash_computations: 0,
            write_at_count: 0,
            read_at_count: 0,
            append_count: 0,
            truncate_count: 0,
        }
    }
}

impl fmt::Display for InlineDataStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InlineDataStats {{ to_disk: {}, from_disk: {}, crc_err: {}, \
             hashes: {}, writes: {}, reads: {}, appends: {}, truncates: {} }}",
            self.to_disk_count,
            self.from_disk_count,
            self.checksum_errors,
            self.hash_computations,
            self.write_at_count,
            self.read_at_count,
            self.append_count,
            self.truncate_count,
        )
    }
}

// ── Helpers internes ───────────────────────────────────────────────────────────

/// Calcule le CRC32 d'un `InlineDataDisk` (bytes [0..564], hors champ checksum).
/// Le champ `checksum` est aux bytes 4..8, donc on calcule sur [0..4] ++ [8..568].
fn inline_crc32(d: &InlineDataDisk) -> u32 {
    let bytes: &[u8; 568] =
        // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
        unsafe { &*(d as *const InlineDataDisk as *const [u8; 568]) };
    // On exclut les 4 octets du checksum (offset 4..8).
    let mut crc = crc32_compute(&bytes[0..4]);
    // Pour simplifier le chaînage on refait le calcul complet en excluant le champ.
    // Implémentation correcte : on skeep bytes[4..8] et on continue sur [8..568].
    crc = crc32_continue(crc, &bytes[8..568]);
    crc
}

/// Continue un calcul CRC32 (state passé en entrée).
fn crc32_continue(mut crc: u32, data: &[u8]) -> u32 {
    crc ^= 0xFFFF_FFFF; // Dé-finaliser.
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_slice_too_large() {
        let big = [0u8; INLINE_DATA_MAX + 1];
        assert!(InlineData::from_slice(&big).is_err());
    }

    #[test]
    fn test_write_read_roundtrip() {
        let mut d = InlineData::empty();
        d.write_at(0, b"hello world").unwrap();
        let mut out = [0u8; 11];
        let n = d.read_at(0, &mut out).unwrap();
        assert_eq!(n, 11);
        assert_eq!(&out, b"hello world");
    }

    #[test]
    fn test_append_and_truncate() {
        let mut d = InlineData::from_slice(b"abc").unwrap();
        d.append(b"def").unwrap();
        assert_eq!(d.as_slice(), b"abcdef");
        d.truncate(3).unwrap();
        assert_eq!(d.as_slice(), b"abc");
    }

    #[test]
    fn test_hash_invalidated_on_update() {
        let mut d = InlineData::from_slice(b"data").unwrap();
        d.compute_hash();
        assert!(d.hash_valid);
        d.update(b"other").unwrap();
        assert!(!d.hash_valid);
    }

    #[test]
    fn test_ct_eq() {
        let d = InlineData::from_slice(b"secret").unwrap();
        assert!(d.ct_eq(b"secret"));
        assert!(!d.ct_eq(b"other"));
    }

    #[test]
    fn test_zeroize() {
        let mut d = InlineData::from_slice(b"sensitive data").unwrap();
        d.zeroize();
        assert!(d.is_empty());
        assert_eq!(d.as_slice(), b"");
    }

    #[test]
    fn test_overflow_protection() {
        let mut d = InlineData::empty();
        let big = [0u8; INLINE_DATA_MAX];
        d.update(&big).unwrap();
        assert!(d.append(b"x").is_err()); // Dépasse capacité.
    }
}
