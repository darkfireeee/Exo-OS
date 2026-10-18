//! # exo-pkg — Format de paquet natif Exo-OS et vérification d'intégrité
//!
//! Spécification: docs/Vision v0.2.0/SPEC-EXO-PKG-STRATA.md
//!
//! Les paquets Exo-OS reposent sur une archive signée contenant un manifeste
//! déclaratif obligatoire (`PackageManifest`) et un ensemble de blobs ExoFS (`ExoAR`).
//!
//! Invariants respectés :
//! - Sécurité fail-closed : rejet de tout paquet sans signature valide ou avec manifeste corrompu.
//! - Règle SRV-04 : délégation des calculs de signature Ed25519 au `crypto_server`.
//! - SRV-02 : #![no_std] strict, zéro allocation sauvage.

#![no_std]

pub mod manifest;
pub mod verify;

pub use manifest::{
    PackageManifest,
    PKG_CAP_CRYPTO, PKG_CAP_DISPLAY, PKG_CAP_IPC, PKG_CAP_NETWORK, PKG_CAP_RAW_IO, PKG_CAP_STORAGE,
    PLEDGE_CPATH, PLEDGE_EXEC, PLEDGE_INET, PLEDGE_RPATH, PLEDGE_STDIO, PLEDGE_UNIX, PLEDGE_WPATH,
};
pub use verify::verify_package_signature;

/// Magic d'en-tête de paquet Exo-OS ("EXOPKG_0" en little-endian)
pub const EXOPKG_MAGIC: u64 = 0x305F_474B_504F_5845;
pub const EXOPKG_VERSION: u16 = 1;

/// Erreurs de gestion des paquets Exo-OS
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PkgError {
    InvalidPackageMagic,
    UnsupportedVersion(u16),
    ManifestTooShort,
    InvalidManifestMagic,
    MalformedManifest,
    SignatureVerificationFailed,
    CryptoServerError(i64),
    CorruptArchive,
}

/// En-tête binaire d'un paquet natif Exo-OS (128 octets)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PackageHeader {
    pub magic: u64,
    pub version: u16,
    pub flags: u16,
    pub manifest_offset: u32,
    pub manifest_len: u32,
    pub payload_offset: u64,
    pub payload_len: u64,
    pub signature: [u8; 64],
    pub signer_pubkey: [u8; 32],
}

impl PackageHeader {
    pub const fn empty() -> Self {
        Self {
            magic: EXOPKG_MAGIC,
            version: EXOPKG_VERSION,
            flags: 0,
            manifest_offset: 0,
            manifest_len: 0,
            payload_offset: 0,
            payload_len: 0,
            signature: [0u8; 64],
            signer_pubkey: [0u8; 32],
        }
    }

    /// Valide l'en-tête du paquet
    pub fn validate(&self) -> Result<(), PkgError> {
        if self.magic != EXOPKG_MAGIC {
            return Err(PkgError::InvalidPackageMagic);
        }
        if self.version != EXOPKG_VERSION {
            return Err(PkgError::UnsupportedVersion(self.version));
        }
        Ok(())
    }
}
