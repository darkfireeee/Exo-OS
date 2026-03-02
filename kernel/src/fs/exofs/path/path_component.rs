// path/path_component.rs — Composant de chemin validé
// Ring 0, no_std
//
// RÈGLES :
//   • NAME_MAX = 255 octets maximum
//   • UTF-8 valide
//   • Pas de slash ('/')
//   • Pas de null byte

use crate::fs::exofs::core::{ExofsError, NAME_MAX};
use core::fmt;

/// Composant de chemin validé — jamais plus long que NAME_MAX,
/// jamais vide après validation, garanti UTF-8 sans slash.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathComponent {
    bytes: [u8; NAME_MAX + 1], // stockage fixe, pas d'allocation heap
    len: u16,
}

impl PathComponent {
    /// Retourne les octets bruts du composant
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Longueur en octets
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Vrai si composant vide (ne devrait jamais arriver après validate_component)
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Construit depuis bytes validés (interne)
    fn from_validated(bytes: &[u8]) -> Self {
        debug_assert!(bytes.len() <= NAME_MAX);
        let mut storage = [0u8; NAME_MAX + 1];
        storage[..bytes.len()].copy_from_slice(bytes);
        PathComponent {
            bytes: storage,
            len: bytes.len() as u16,
        }
    }

    /// Hash FNV-1a du composant (utilisé par PathIndex)
    pub fn fnv_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &b in self.as_bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }
}

impl fmt::Display for PathComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Essaie d'afficher en UTF-8, sinon en hex
        match core::str::from_utf8(self.as_bytes()) {
            Ok(s) => write!(f, "{}", s),
            Err(_) => {
                write!(f, "[")?;
                for b in self.as_bytes() {
                    write!(f, "{:02x}", b)?;
                }
                write!(f, "]")
            }
        }
    }
}

// ─── Validation ───────────────────────────────────────────────────────────────

/// Valide et retourne un composant de chemin.
///
/// # Erreurs
/// - `ExofsError::NameTooLong` si len > NAME_MAX
/// - `ExofsError::InvalidPathComponent` si contient '/' ou '\0', ou est vide
/// - `ExofsError::InvalidMagic` si UTF-8 invalide (recyclé pour simplifier)
pub fn validate_component(bytes: &[u8]) -> Result<PathComponent, ExofsError> {
    if bytes.is_empty() {
        return Err(ExofsError::InvalidPathComponent);
    }
    if bytes.len() > NAME_MAX {
        return Err(ExofsError::NameTooLong);
    }
    for &b in bytes {
        if b == b'/' || b == 0 {
            return Err(ExofsError::InvalidPathComponent);
        }
    }
    // Validation UTF-8
    core::str::from_utf8(bytes).map_err(|_| ExofsError::InvalidPathComponent)?;

    Ok(PathComponent::from_validated(bytes))
}

/// Vérifie si un composant est '.' (répertoire courant)
#[inline]
pub fn is_dot(comp: &PathComponent) -> bool {
    comp.as_bytes() == b"."
}

/// Vérifie si un composant est '..' (répertoire parent)
#[inline]
pub fn is_dotdot(comp: &PathComponent) -> bool {
    comp.as_bytes() == b".."
}
