// SPDX-License-Identifier: MIT
// ExoFS — object_kind/secret.rs
// SecretDescriptor — objets chiffrés à accès contrôlé.
//
// Règles :
//   SEC-03 : BlobId d'un Secret = Blake3(plaintext) AVANT chiffrement
//   SEC-04 : contenu jamais loggué, jamais dans les stats
//   ONDISK-01 : SecretDescriptorDisk #[repr(C, packed)]
//   ARITH-02  : checked_add / saturating_* partout

use core::fmt;
use core::mem;

use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::core::{
    blake3_hash, compute_blob_id, BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, ObjectId,
};

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Magic d'un SecretDescriptorDisk.
pub const SECRET_DESCRIPTOR_MAGIC: u32 = 0x5EC4_E700;

/// Version du format SecretDescriptorDisk.
pub const SECRET_DESCRIPTOR_VERSION: u8 = 1;

/// Taille maximale d'un objet Secret (16 Mio) — assez pour des clés et certificats.
pub const SECRET_MAX_SIZE: u64 = 16 * 1024 * 1024;

/// Longueur d'une nonce pour AES-256-GCM.
pub const SECRET_NONCE_LEN: usize = 12;

/// Longueur d'un tag d'authentification GCM.
pub const SECRET_AUTH_TAG_LEN: usize = 16;

/// Longueur d'une clé dérivée (KEK hash).
pub const SECRET_KEY_ID_LEN: usize = 32;

// ── Algorithmes de chiffrement ─────────────────────────────────────────────────

/// Algorithme de chiffrement supporté.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SecretCipher {
    /// AES-256-GCM (AEAD).
    Aes256Gcm = 0x01,
    /// ChaCha20-Poly1305 (AEAD).
    ChaCha20Poly1305 = 0x02,
    Unknown = 0xFF,
}

impl SecretCipher {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => Self::Aes256Gcm,
            0x02 => Self::ChaCha20Poly1305,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for SecretCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aes256Gcm => write!(f, "AES-256-GCM"),
            Self::ChaCha20Poly1305 => write!(f, "ChaCha20-Poly1305"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

// ── SecretDescriptorDisk ───────────────────────────────────────────────────────

/// Représentation on-disk d'un descripteur Secret (192 octets, ONDISK-01).
///
/// Layout :
/// ```text
///   0..  3  magic           u32
///   4.. 35  plaintext_bid   [u8;32]  — Blake3(plaintext) = BlobId SEC-03
///  36.. 67  object_id       [u8;32]
///  68.. 75  disk_offset     u64      — offset du payload chiffré
///  76.. 83  ciphertext_size u64      — taille du payload chiffré
///  84.. 91  plaintext_size  u64      — taille originale (AVANT chiffrement)
///  92..103  nonce           [u8;12]  — nonce AEAD
/// 104..119  auth_tag        [u8;16]  — tag AEAD
/// 120..151  key_id          [u8;32]  — hash de la KEK utilisée
/// 152..159  epoch_create    u64
/// 160..161  flags           u16
/// 162       cipher          u8
/// 163       version         u8
/// 164..175  _pad            [u8;12]
/// 176..191  checksum        [u8;16]  — Blake3(176 premiers octets), tronqué
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct SecretDescriptorDisk {
    pub magic: u32,
    pub plaintext_bid: [u8; 32], // BlobId du plaintext (SEC-03)
    pub object_id: [u8; 32],
    pub disk_offset: u64,
    pub ciphertext_size: u64,
    pub plaintext_size: u64,
    pub nonce: [u8; SECRET_NONCE_LEN],
    pub auth_tag: [u8; SECRET_AUTH_TAG_LEN],
    pub key_id: [u8; SECRET_KEY_ID_LEN],
    pub epoch_create: u64,
    pub flags: u16,
    pub cipher: u8,
    pub version: u8,
    pub _pad: [u8; 12],
    pub checksum: [u8; 16],
}

const _: () = assert!(
    mem::size_of::<SecretDescriptorDisk>() == 192,
    "SecretDescriptorDisk doit être 192 octets (ONDISK-01)"
);

impl SecretDescriptorDisk {
    pub fn compute_checksum(&self) -> [u8; 16] {
        let raw: &[u8; 192] =
            // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
            unsafe { &*(self as *const SecretDescriptorDisk as *const [u8; 192]) };
        let full = blake3_hash(&raw[..176]);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }

    pub fn verify(&self) -> ExofsResult<()> {
        if self.magic != SECRET_DESCRIPTOR_MAGIC {
            return Err(ExofsError::Corrupt);
        }
        if self.version != SECRET_DESCRIPTOR_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        if matches!(SecretCipher::from_u8(self.cipher), SecretCipher::Unknown) {
            return Err(ExofsError::InvalidArgument);
        }
        let computed = self.compute_checksum();
        if self.checksum != computed {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }
}

impl fmt::Debug for SecretDescriptorDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SEC-04 : ne jamais loguer le contenu, nonce ou auth_tag.
        write!(
            f,
            "SecretDescriptorDisk {{ cipher: {}, flags: {:#x} }}",
            { self.cipher },
            { self.flags },
        )
    }
}

// ── SecretAccessRecord ──────────────────────────────────────────────────────────

/// Enregistrement d'un accès à un Secret (audit trail minimal, SEC-04).
///
/// Ne stocke JAMAIS le contenu, ni la clé, seulement les IDs et epochs.
#[derive(Copy, Clone, Debug)]
pub struct SecretAccessRecord {
    /// ID de l'objet Secret accédé.
    pub object_id: ObjectId,
    /// Epoch de l'accès.
    pub epoch_access: EpochId,
    /// Hash de l'identité du demandeur (capability hash).
    pub accessor_hash: [u8; 32],
    /// Vrai si la lecture a réussi.
    pub success: bool,
}

impl SecretAccessRecord {
    pub fn new(
        object_id: ObjectId,
        epoch: EpochId,
        accessor_hash: [u8; 32],
        success: bool,
    ) -> Self {
        Self {
            object_id,
            epoch_access: epoch,
            accessor_hash,
            success,
        }
    }
}

impl fmt::Display for SecretAccessRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SEC-04 : ne pas loguer accessor_hash en clair.
        write!(
            f,
            "SecretAccess {{ object: {:02x?}, epoch: {}, ok: {} }}",
            &self.object_id.0[..4],
            self.epoch_access.0,
            self.success,
        )
    }
}

// ── SecretDescriptor in-memory ─────────────────────────────────────────────────

/// Descripteur in-memory d'un objet Secret ExoFS.
///
/// Le contenu chiffré reste sur disque ; ici on garde uniquement les métadonnées.
/// SEC-04 : aucun champ ne contient le plaintext ou la clé de chiffrement.
pub struct SecretDescriptor {
    /// BlobId du plaintext (AVANT chiffrement), SEC-03.
    pub plaintext_bid: BlobId,
    /// Objet propriétaire.
    pub object_id: ObjectId,
    /// Offset disque du payload chiffré.
    pub disk_offset: DiskOffset,
    /// Taille du ciphertext (stocké sur disque).
    pub ciphertext_size: u64,
    /// Taille du plaintext original.
    pub plaintext_size: u64,
    /// Nonce AEAD (12 octets pour AES-256-GCM).
    pub nonce: [u8; SECRET_NONCE_LEN],
    /// Tag d'authentification AEAD.
    pub auth_tag: [u8; SECRET_AUTH_TAG_LEN],
    /// ID de la KEK (Key Encryption Key) utilisée.
    pub key_id: [u8; SECRET_KEY_ID_LEN],
    /// Epoch de création.
    pub epoch_create: EpochId,
    /// Flags de l'objet (doit inclure ENCRYPTED).
    pub flags: u16,
    /// Algorithme de chiffrement.
    pub cipher: SecretCipher,
}

impl SecretDescriptor {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    /// Crée un descripteur Secret.
    ///
    /// `plaintext_bid` doit être calculé par l'appelant sur le texte clair
    /// AVANT chiffrement (SEC-03).
    ///
    /// Le ciphertext est déjà chiffré et positionné sur disque par l'appelant.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plaintext_bid: BlobId,
        object_id: ObjectId,
        disk_offset: DiskOffset,
        ciphertext_size: u64,
        plaintext_size: u64,
        nonce: [u8; SECRET_NONCE_LEN],
        auth_tag: [u8; SECRET_AUTH_TAG_LEN],
        key_id: [u8; SECRET_KEY_ID_LEN],
        epoch_create: EpochId,
        cipher: SecretCipher,
    ) -> ExofsResult<Self> {
        if plaintext_size > SECRET_MAX_SIZE {
            return Err(ExofsError::Overflow);
        }
        if matches!(cipher, SecretCipher::Unknown) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self {
            plaintext_bid,
            object_id,
            disk_offset,
            ciphertext_size,
            plaintext_size,
            nonce,
            auth_tag,
            key_id,
            epoch_create,
            flags: ObjectFlags::ENCRYPTED.0,
            cipher,
        })
    }

    /// Reconstruit depuis on-disk (HDR-03 : verify() en premier).
    pub fn from_disk(d: &SecretDescriptorDisk) -> ExofsResult<Self> {
        d.verify()?;
        let cipher = SecretCipher::from_u8(d.cipher);
        Ok(Self {
            plaintext_bid: BlobId(d.plaintext_bid),
            object_id: ObjectId(d.object_id),
            disk_offset: DiskOffset(d.disk_offset),
            ciphertext_size: d.ciphertext_size,
            plaintext_size: d.plaintext_size,
            nonce: d.nonce,
            auth_tag: d.auth_tag,
            key_id: d.key_id,
            epoch_create: EpochId(d.epoch_create),
            flags: d.flags,
            cipher,
        })
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    pub fn to_disk(&self) -> SecretDescriptorDisk {
        let mut d = SecretDescriptorDisk {
            magic: SECRET_DESCRIPTOR_MAGIC,
            plaintext_bid: self.plaintext_bid.0,
            object_id: self.object_id.0,
            disk_offset: self.disk_offset.0,
            ciphertext_size: self.ciphertext_size,
            plaintext_size: self.plaintext_size,
            nonce: self.nonce,
            auth_tag: self.auth_tag,
            key_id: self.key_id,
            epoch_create: self.epoch_create.0,
            flags: self.flags,
            cipher: self.cipher as u8,
            version: SECRET_DESCRIPTOR_VERSION,
            _pad: [0; 12],
            checksum: [0; 16],
        };
        d.checksum = d.compute_checksum();
        d
    }

    // ── SEC-03 : vérification du BlobId sur plaintext ──────────────────────────

    /// Vérifie que `plaintext` correspond au BlobId enregistré (SEC-03).
    ///
    /// SEC-04 : le plaintext n'est jamais loggué.
    pub fn verify_plaintext_id(&self, plaintext: &[u8]) -> ExofsResult<()> {
        let computed = compute_blob_id(plaintext);
        if computed != self.plaintext_bid {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }

    /// Vrai si l'objet a le flag ENCRYPTED (doit toujours être vrai).
    #[inline]
    pub fn is_encrypted(&self) -> bool {
        self.flags & ObjectFlags::ENCRYPTED.0 != 0
    }

    // ── Validation ────────────────────────────────────────────────────────────

    pub fn validate(&self) -> ExofsResult<()> {
        if !self.is_encrypted() {
            // Un Secret sans flag ENCRYPTED est incohérent.
            return Err(ExofsError::Corrupt);
        }
        if self.plaintext_size > SECRET_MAX_SIZE {
            return Err(ExofsError::Overflow);
        }
        if matches!(self.cipher, SecretCipher::Unknown) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// SEC-04 : Display ne montre jamais le contenu, nonce, ni la clé.
impl fmt::Display for SecretDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SecretDescriptor {{ cipher: {}, size: {} B, encrypted: {} }}",
            self.cipher,
            self.plaintext_size,
            self.is_encrypted(),
        )
    }
}

impl fmt::Debug for SecretDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── Fonctions utilitaires publiques ────────────────────────────────────────────

/// Vérifie que les flags d'un objet sont cohérents avec un objet Secret.
///
/// DOIT inclure ENCRYPTED.
pub fn secret_flags_valid(flags: ObjectFlags) -> bool {
    flags.contains(ObjectFlags::ENCRYPTED)
}

/// Calcule le BlobId du plaintext (SEC-03 : AVANT chiffrement).
///
/// SEC-04 : données JAMAIS loguées.
pub fn secret_compute_plaintext_id(plaintext: &[u8]) -> BlobId {
    compute_blob_id(plaintext)
}

// ── SecretStats ──────────────────────────────────────────────────────────────────

/// Statistiques agrégées des objets Secret.
///
/// SEC-04 : aucune statistique de contenu.
#[derive(Default, Debug)]
pub struct SecretStats {
    pub total: u64,
    pub aes256gcm_count: u64,
    pub chacha20poly_count: u64,
    pub total_cipher_bytes: u64, // taille aggregate du ciphertext
    pub access_deny_count: u64,
    pub access_grant_count: u64,
}

impl SecretStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, s: &SecretDescriptor) {
        self.total = self.total.saturating_add(1);
        self.total_cipher_bytes = self.total_cipher_bytes.saturating_add(s.ciphertext_size);
        match s.cipher {
            SecretCipher::Aes256Gcm => {
                self.aes256gcm_count = self.aes256gcm_count.saturating_add(1)
            }
            SecretCipher::ChaCha20Poly1305 => {
                self.chacha20poly_count = self.chacha20poly_count.saturating_add(1)
            }
            _ => {}
        }
    }

    pub fn record_access(&mut self, rec: &SecretAccessRecord) {
        if rec.success {
            self.access_grant_count = self.access_grant_count.saturating_add(1);
        } else {
            self.access_deny_count = self.access_deny_count.saturating_add(1);
        }
    }
}

impl fmt::Display for SecretStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SecretStats {{ total: {}, aes: {}, cha: {}, \
             grants: {}, denies: {} }}",
            self.total,
            self.aes256gcm_count,
            self.chacha20poly_count,
            self.access_grant_count,
            self.access_deny_count,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_descriptor_disk_size() {
        assert_eq!(mem::size_of::<SecretDescriptorDisk>(), 192);
    }

    #[test]
    fn test_secret_flags_valid() {
        let good = ObjectFlags(ObjectFlags::ENCRYPTED.0);
        let bad = ObjectFlags(0);
        assert!(secret_flags_valid(good));
        assert!(!secret_flags_valid(bad));
    }

    #[test]
    fn test_plaintext_id_mismatch() {
        let data = b"my secret";
        let bid = secret_compute_plaintext_id(data);
        let s = SecretDescriptor::new(
            bid,
            ObjectId([0; 32]),
            DiskOffset(0),
            100,
            data.len() as u64,
            [0u8; 12],
            [0u8; 16],
            [0u8; 32],
            EpochId(1),
            SecretCipher::Aes256Gcm,
        )
        .unwrap();
        assert!(s.verify_plaintext_id(data).is_ok());
        assert!(s.verify_plaintext_id(b"tampered").is_err());
    }

    #[test]
    fn test_to_disk_roundtrip() {
        let data = b"secret payload";
        let bid = secret_compute_plaintext_id(data);
        let orig = SecretDescriptor::new(
            bid,
            ObjectId([1; 32]),
            DiskOffset(8192),
            200,
            data.len() as u64,
            [0u8; 12],
            [0xFF; 16],
            [0xAB; 32],
            EpochId(5),
            SecretCipher::ChaCha20Poly1305,
        )
        .unwrap();
        let disk = orig.to_disk();
        disk.verify().expect("verify doit réussir");
        let back = SecretDescriptor::from_disk(&disk).unwrap();
        assert_eq!(back.plaintext_size, data.len() as u64);
        assert!(matches!(back.cipher, SecretCipher::ChaCha20Poly1305));
    }
}
