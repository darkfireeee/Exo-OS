//! manifest.rs — Parseur de manifeste de paquet natif Exo-OS (#![no_std])
//!
//! Spécification: docs/Vision v0.2.0/SPEC-EXO-PKG-STRATA.md
//! Déclare obligatoirement les Capabilities et les règles de Sandbox (pledge / exocage).

use crate::PkgError;

// Drapeaux de capacités (Capabilities) déclarées dans le manifeste
pub const PKG_CAP_NETWORK: u32 = 1 << 0;
pub const PKG_CAP_STORAGE: u32 = 1 << 1;
pub const PKG_CAP_IPC: u32 = 1 << 2;
pub const PKG_CAP_DISPLAY: u32 = 1 << 3;
pub const PKG_CAP_RAW_IO: u32 = 1 << 4;
pub const PKG_CAP_CRYPTO: u32 = 1 << 5;

// Drapeaux de sandbox et confinement
pub const PLEDGE_STDIO: u32 = 1 << 0;
pub const PLEDGE_RPATH: u32 = 1 << 1;
pub const PLEDGE_WPATH: u32 = 1 << 2;
pub const PLEDGE_CPATH: u32 = 1 << 3;
pub const PLEDGE_INET: u32 = 1 << 4;
pub const PLEDGE_UNIX: u32 = 1 << 5;
pub const PLEDGE_EXEC: u32 = 1 << 6;

/// Manifeste d'un paquet applicatif ou système Exo-OS
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageManifest {
    pub name: [u8; 32],
    pub name_len: usize,
    pub version: (u16, u16, u16),
    pub capabilities: u32,
    pub pledge_flags: u32,
    pub exocage_enabled: bool,
    pub entrypoint: [u8; 64],
    pub entrypoint_len: usize,
}

impl PackageManifest {
    pub const fn empty() -> Self {
        Self {
            name: [0u8; 32],
            name_len: 0,
            version: (0, 0, 0),
            capabilities: 0,
            pledge_flags: 0,
            exocage_enabled: true, // Fail-closed: sandbox active par défaut
            entrypoint: [0u8; 64],
            entrypoint_len: 0,
        }
    }

    /// Analyse et valide un buffer de manifeste au format binaire ou clé-valeur
    pub fn parse(bytes: &[u8]) -> Result<Self, PkgError> {
        // AUDIT-LIB2 : l'en-tête fixe nécessite au moins 20 octets (champs jusqu'à bytes[19])
        if bytes.len() < 20 {
            return Err(PkgError::ManifestTooShort);
        }

        let mut manifest = Self::empty();

        // Format compact: [4B magic "EXPM"][2B major][2B minor][2B patch][4B caps][4B pledge][1B exocage][1B name_len][name..]
        if bytes[0..4] != [b'E', b'X', b'P', b'M'] {
            return Err(PkgError::InvalidManifestMagic);
        }

        manifest.version.0 = u16::from_le_bytes([bytes[4], bytes[5]]);
        manifest.version.1 = u16::from_le_bytes([bytes[6], bytes[7]]);
        manifest.version.2 = u16::from_le_bytes([bytes[8], bytes[9]]);

        manifest.capabilities = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        manifest.pledge_flags = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
        manifest.exocage_enabled = bytes[18] != 0;

        let name_len = bytes[19] as usize;
        if 20 + name_len > bytes.len() || name_len > 32 {
            return Err(PkgError::MalformedManifest);
        }

        manifest.name[..name_len].copy_from_slice(&bytes[20..20 + name_len]);
        manifest.name_len = name_len;

        Ok(manifest)
    }
}
