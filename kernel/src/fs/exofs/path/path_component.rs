//! path_component.rs — Composant de chemin ExoFS validé.
//!
//! Un [`PathComponent`] représente un segment individuel d un chemin de système
//! de fichiers (entre deux séparateurs `/`). Il est garanti :
//! - Non vide.
//! - Longueur ≤ [`NAME_MAX`] (255 octets).
//! - Sans octet nul (`\0`) ni slash (`/`).
//! - Valide en UTF-8.
//!
//! Stockage fixe sur la pile — aucune allocation heap pour le composant lui-même.
//! Le parseur [`PathParser`] découpe un chemin complet en composants de façon
//! **itérative** (règle RECUR-01).
//!
//! # Règles spec appliquées
//! - **RECUR-01** : parsing itératif, jamais récursif.
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push`.
//! - **ARITH-02** : `checked_add` sur les offsets de lecture.

extern crate alloc;
use alloc::vec::Vec;

/// Longueur maximale d'un chemin complet ExoFS (en octets, nul exclu).
/// Conforme POSIX PATH_MAX = 4096.
pub const PATH_MAX: usize = 4096;
use core::fmt;

use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Longueur maximale d un composant de chemin (255 octets, POSIX NAME_MAX).
pub const NAME_MAX: usize = 255;

// ── PathComponent ─────────────────────────────────────────────────────────────

/// Composant de chemin validé — stocké sur la pile, jamais sur le heap.
///
/// Taille : `NAME_MAX + 3` octets (stockage + longueur u16).
/// Garanties post-construction :
/// - Pas vide.
/// - Pas de `b'/'` ni de `b'\0'`.
/// - Valide UTF-8.
/// - Longueur ≤ `NAME_MAX`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathComponent {
    bytes: [u8; NAME_MAX + 1],
    len: u16,
}

impl PathComponent {
    /// Retourne les octets bruts du composant.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Longueur en octets.
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// `true` si le composant est vide (ne devrait jamais arriver après validation).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` si le composant est `.` (répertoire courant).
    #[inline]
    pub fn is_dot(&self) -> bool {
        self.as_bytes() == b"."
    }

    /// `true` si le composant est `..` (répertoire parent).
    #[inline]
    pub fn is_dotdot(&self) -> bool {
        self.as_bytes() == b".."
    }

    /// `true` si le composant est `.` ou `..`.
    #[inline]
    pub fn is_special(&self) -> bool {
        self.is_dot() || self.is_dotdot()
    }

    /// Vue `&str` du composant (garanti UTF-8 après validation).
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: `validate_component` garantit UTF-8 valide.
        unsafe { core::str::from_utf8_unchecked(self.as_bytes()) }
    }

    /// Hash FNV-1a du composant (utilisé par `PathIndex` pour le tri).
    pub fn fnv1a(&self) -> u64 {
        fnv1a_hash(self.as_bytes())
    }

    /// Comparaison insensible à la casse ASCII (pour systèmes compatible Windows).
    pub fn eq_ignore_ascii_case(&self, other: &PathComponent) -> bool {
        let a = self.as_bytes();
        let b = other.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter()
            .zip(b.iter())
            .all(|(&x, &y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
    }

    /// Construit depuis des octets déjà validés (usage interne).
    pub(crate) fn from_validated(bytes: &[u8]) -> Self {
        debug_assert!(!bytes.is_empty());
        debug_assert!(bytes.len() <= NAME_MAX);
        let mut storage = [0u8; NAME_MAX + 1];
        storage[..bytes.len()].copy_from_slice(bytes);
        PathComponent {
            bytes: storage,
            len: bytes.len() as u16,
        }
    }

    /// Retourne une copie dans un tableau de taille fixe (utile pour les clés de BTreeMap).
    pub fn to_fixed_bytes(&self) -> ([u8; NAME_MAX + 1], u16) {
        (self.bytes, self.len)
    }
}

impl fmt::Display for PathComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match core::str::from_utf8(self.as_bytes()) {
            Ok(s) => write!(f, "{}", s),
            Err(_) => {
                write!(f, "<")?;
                for b in self.as_bytes() {
                    write!(f, "{:02x}", b)?;
                }
                write!(f, ">")
            }
        }
    }
}

impl PartialOrd for PathComponent {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathComponent {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Valide et retourne un composant de chemin depuis des octets bruts.
///
/// # Règles de validation (dans l ordre)
/// 1. Non vide.
/// 2. Longueur ≤ `NAME_MAX`.
/// 3. Pas de `b'/'` ni de `b'\0'`.
/// 4. Valide UTF-8.
///
/// # Errors
/// - [`ExofsError::InvalidPathComponent`] si vide, contient `/`/`\0`, ou UTF-8 invalide.
/// - [`ExofsError::PathTooLong`] si longueur > `NAME_MAX`.
pub fn validate_component(bytes: &[u8]) -> ExofsResult<PathComponent> {
    if bytes.is_empty() {
        return Err(ExofsError::InvalidPathComponent);
    }
    if bytes.len() > NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    for &b in bytes {
        if b == b'/' || b == 0 {
            return Err(ExofsError::InvalidPathComponent);
        }
    }
    core::str::from_utf8(bytes).map_err(|_| ExofsError::InvalidPathComponent)?;
    Ok(PathComponent::from_validated(bytes))
}

/// Valide un composant sans construire la structure (vérification rapide).
pub fn is_valid_component(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > NAME_MAX {
        return false;
    }
    if bytes.iter().any(|&b| b == b'/' || b == 0) {
        return false;
    }
    core::str::from_utf8(bytes).is_ok()
}

// ── PathParser — découpage itératif (RECUR-01) ────────────────────────────────

/// Parseur itératif d un chemin complet.
///
/// Découpe un chemin (ex : `/home/user/file.txt`) en composants validés.
/// N est **jamais** récursif (règle RECUR-01).
///
/// # Usage
/// ```ignore
/// let mut parser = PathParser::new(b"/home/user/file");
/// while let Some(comp) = parser.next_component()? { ... }
/// ```
pub struct PathParser<'a> {
    /// Source du chemin.
    src: &'a [u8],
    /// Position courante dans `src`.
    pos: usize,
    /// Indique si le chemin était absolu (commençait par `/`).
    is_abs: bool,
    /// Composants restants à produire.
    done: bool,
}

impl<'a> PathParser<'a> {
    /// Crée un parseur pour le chemin `path`.
    ///
    /// # Errors
    /// - [`ExofsError::InvalidPathComponent`] si `path` est vide.
    /// - [`ExofsError::PathTooLong`] si `path` dépasse 4096 octets.
    pub fn new(path: &'a [u8]) -> ExofsResult<Self> {
        if path.is_empty() {
            return Err(ExofsError::InvalidPathComponent);
        }
        if path.len() > 4096 {
            return Err(ExofsError::PathTooLong);
        }
        let is_abs = path.first() == Some(&b'/');
        let start = if is_abs { 1 } else { 0 };
        Ok(PathParser {
            src: path,
            pos: start,
            is_abs,
            done: false,
        })
    }

    /// `true` si le chemin est absolu (commence par `/`).
    #[inline]
    pub fn is_absolute(&self) -> bool {
        self.is_abs
    }

    /// `true` si tous les composants ont été produits.
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.done
    }

    /// Retourne le prochain composant validé, ou `None` en fin de chemin.
    ///
    /// # Errors
    /// Propage les erreurs de [`validate_component`].
    pub fn next_component(&mut self) -> ExofsResult<Option<PathComponent>> {
        if self.done {
            return Ok(None);
        }

        // Sauter les slashes consécutifs.
        while self.pos < self.src.len() && self.src[self.pos] == b'/' {
            self.pos = self.pos.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }

        if self.pos >= self.src.len() {
            self.done = true;
            return Ok(None);
        }

        // Trouver la fin du composant.
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] != b'/' {
            self.pos = self.pos.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }

        let seg = &self.src[start..self.pos];
        if seg.is_empty() {
            self.done = true;
            return Ok(None);
        }

        let comp = validate_component(seg)?;
        Ok(Some(comp))
    }

    /// Collecte tous les composants restants dans un `Vec`.
    ///
    /// # OOM-02
    /// `try_reserve(1)` avant chaque `push`.
    pub fn collect_all(&mut self) -> ExofsResult<Vec<PathComponent>> {
        let mut out = Vec::new();
        loop {
            match self.next_component()? {
                None => break,
                Some(c) => {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    /// Retourne le nombre de slashes restants (estimation pour la profondeur).
    pub fn remaining_depth(&self) -> usize {
        if self.done {
            return 0;
        }
        self.src[self.pos..]
            .iter()
            .filter(|&&b| b == b'/')
            .count()
            .saturating_add(1)
    }
}

impl<'a> Iterator for PathParser<'a> {
    type Item = crate::fs::exofs::core::ExofsResult<PathComponent>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.next_component() {
            Ok(Some(c)) => Some(Ok(c)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

// ── PathComponentBuf — vecteur de composants ──────────────────────────────────

/// Vecteur de composants de chemin avec méthodes de manipulation.
///
/// Toutes les modifications respectent **OOM-02** (`try_reserve(1)` avant push).
#[derive(Clone, Debug, Default)]
pub struct PathComponentBuf {
    components: Vec<PathComponent>,
}

impl PathComponentBuf {
    /// Crée un buffer vide.
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    /// Crée un buffer pré-dimensionné.
    ///
    /// # OOM-02
    pub fn with_capacity(cap: usize) -> ExofsResult<Self> {
        let mut v = Vec::new();
        v.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { components: v })
    }

    /// Parse un chemin complet et remplit le buffer.
    pub fn from_path(path: &[u8]) -> ExofsResult<Self> {
        let mut parser = PathParser::new(path)?;
        let comps = parser.collect_all()?;
        Ok(Self { components: comps })
    }

    /// Ajoute un composant (OOM-02).
    pub fn push(&mut self, comp: PathComponent) -> ExofsResult<()> {
        self.components
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.components.push(comp);
        Ok(())
    }

    /// Retire le dernier composant (`..` sémantique).
    pub fn pop(&mut self) {
        self.components.pop();
    }

    /// Nombre de composants.
    #[inline]
    pub fn len(&self) -> usize {
        self.components.len()
    }
    /// `true` si aucun composant.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
    /// Vue slice.
    #[inline]
    pub fn as_slice(&self) -> &[PathComponent] {
        &self.components
    }
    /// Itérateur.
    pub fn iter(&self) -> core::slice::Iter<'_, PathComponent> {
        self.components.iter()
    }

    /// Reconstruction du chemin sous forme d octet (sans allocation pour de petits chemins).
    ///
    /// Max 4096 octets. Retourne `Err(PathTooLong)` si dépassé.
    pub fn to_bytes(&self) -> ExofsResult<Vec<u8>> {
        let mut out: Vec<u8> = Vec::new();
        let mut total: usize = 0;
        for (i, comp) in self.components.iter().enumerate() {
            total = total
                .checked_add(1 + comp.len())
                .ok_or(ExofsError::PathTooLong)?;
            if total > 4096 {
                return Err(ExofsError::PathTooLong);
            }
            out.try_reserve(1 + comp.len())
                .map_err(|_| ExofsError::NoMemory)?;
            if i == 0 {
                out.push(b'/');
            } else {
                out.push(b'/');
            }
            out.extend_from_slice(comp.as_bytes());
        }
        if out.is_empty() {
            out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            out.push(b'/');
        }
        Ok(out)
    }

    /// Applique la sémantique `.` et `..` : normalise le buffer en place.
    pub fn normalize(&mut self) -> ExofsResult<()> {
        let mut result: Vec<PathComponent> = Vec::new();
        for comp in self.components.drain(..) {
            if comp.is_dot() {
                // Ignorer `.`
            } else if comp.is_dotdot() {
                result.pop(); // Remonter au parent.
            } else {
                result.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                result.push(comp);
            }
        }
        self.components = result;
        Ok(())
    }

    /// Retourne le dernier composant (tail / basename).
    pub fn last(&self) -> Option<&PathComponent> {
        self.components.last()
    }

    /// Retourne tous les composants sauf le dernier (dirname).
    pub fn parent(&self) -> &[PathComponent] {
        let len = self.components.len();
        if len == 0 {
            &[]
        } else {
            &self.components[..len - 1]
        }
    }

    /// Étend avec des composants d un autre buffer (OOM-02).
    pub fn extend_from(&mut self, other: &PathComponentBuf) -> ExofsResult<()> {
        self.components
            .try_reserve(other.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for c in other.iter() {
            self.components.push(c.clone());
        }
        Ok(())
    }
}

// ── Utilitaire de hachage ─────────────────────────────────────────────────────

/// Hash FNV-1a rapide (kernel-safe, pas d allocation).
#[inline]
pub fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hash FNV-1a combiné de deux tranches (ex : namespace + composant).
#[inline]
pub fn fnv1a_combine(a: &[u8], b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in a {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= b'/' as u64;
    h = h.wrapping_mul(0x100000001b3);
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hash SipHash-2-4 avec clé de montage (LAC-05/PATH-01/S-12).
///
/// Utiliser à la place de `fnv1a_hash` pour toute indexation de chemins
/// dans un volume monté — `key` provient du `mount_key` de `PathIndex`.
///
/// ⚠️ PATH-02 : `key` doit être généré de manière aléatoire au montage.
/// Ne jamais utiliser `[0u8; 16]` en production.
#[inline]
pub fn siphash_keyed(key: &[u8; 16], data: &[u8]) -> u64 {
    use core::hash::Hasher;
    use siphasher::sip::SipHasher24;
    // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
    let k0 = u64::from_le_bytes(unsafe { *(key[0..8].as_ptr() as *const [u8; 8]) });
    // SAFETY: pointeur calculé depuis une slice dont la longueur a été vérifiée.
    let k1 = u64::from_le_bytes(unsafe { *(key[8..16].as_ptr() as *const [u8; 8]) });
    let mut h = SipHasher24::new_with_keys(k0, k1);
    h.write(data);
    h.finish()
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ok() {
        let c = validate_component(b"hello").unwrap();
        assert_eq!(c.as_bytes(), b"hello");
        assert!(!c.is_dot());
        assert!(!c.is_dotdot());
    }
    #[test]
    fn test_validate_empty() {
        assert!(matches!(
            validate_component(b""),
            Err(ExofsError::InvalidPathComponent)
        ));
    }
    #[test]
    fn test_validate_slash() {
        assert!(matches!(
            validate_component(b"a/b"),
            Err(ExofsError::InvalidPathComponent)
        ));
    }
    #[test]
    fn test_validate_null() {
        assert!(matches!(
            validate_component(b"a\0b"),
            Err(ExofsError::InvalidPathComponent)
        ));
    }
    #[test]
    fn test_validate_too_long() {
        let long = [b'a'; 256];
        assert!(matches!(
            validate_component(&long),
            Err(ExofsError::PathTooLong)
        ));
    }
    #[test]
    fn test_dot_dotdot() {
        assert!(validate_component(b".").unwrap().is_dot());
        assert!(validate_component(b"..").unwrap().is_dotdot());
    }
    #[test]
    fn test_parser_absolute() {
        let mut p = PathParser::new(b"/home/user/file.txt").unwrap();
        assert!(p.is_absolute());
        assert_eq!(p.next_component().unwrap().unwrap().as_bytes(), b"home");
        assert_eq!(p.next_component().unwrap().unwrap().as_bytes(), b"user");
        assert_eq!(p.next_component().unwrap().unwrap().as_bytes(), b"file.txt");
        assert!(p.next_component().unwrap().is_none());
    }
    #[test]
    fn test_parser_relative() {
        let mut p = PathParser::new(b"a/b").unwrap();
        assert!(!p.is_absolute());
        assert_eq!(p.next_component().unwrap().unwrap().as_bytes(), b"a");
        assert_eq!(p.next_component().unwrap().unwrap().as_bytes(), b"b");
    }
    #[test]
    fn test_parser_double_slash() {
        let mut p = PathParser::new(b"//a//b//").unwrap();
        let comps = p.collect_all().unwrap();
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[0].as_bytes(), b"a");
        assert_eq!(comps[1].as_bytes(), b"b");
    }
    #[test]
    fn test_component_buf_normalize() {
        let mut buf = PathComponentBuf::from_path(b"/a/b/../c/./d").unwrap();
        buf.normalize().unwrap();
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.as_slice()[0].as_bytes(), b"a");
        assert_eq!(buf.as_slice()[1].as_bytes(), b"c");
        assert_eq!(buf.as_slice()[2].as_bytes(), b"d");
    }
    #[test]
    fn test_fnv1a_deterministic() {
        let h1 = fnv1a_hash(b"hello");
        let h2 = fnv1a_hash(b"hello");
        assert_eq!(h1, h2);
        assert_ne!(h1, fnv1a_hash(b"world"));
    }
    #[test]
    fn test_component_ordering() {
        let a = validate_component(b"alpha").unwrap();
        let b = validate_component(b"beta").unwrap();
        assert!(a < b);
    }
    #[test]
    fn test_to_bytes_root() {
        let buf = PathComponentBuf::new();
        let bytes = buf.to_bytes().unwrap();
        assert_eq!(bytes, b"/");
    }
}
