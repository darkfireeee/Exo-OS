//! canonicalize.rs -- Normalisation iterative de chemins ExoFS.
//!
//! Supprime les doubles slashes, resout les composants "." et ".."
//! de facon *iterative* (regle RECUR-01). Aucune recursion.
//!
//! ## Regles spec
//! - **RECUR-01** : iteratif -- jamais recursif.
//! - **ARITH-02** : checked_add sur tous les calculs doffset.
//! - **OOM-02** : try_reserve(1) avant push.

extern crate alloc;
use alloc::vec::Vec;

use super::path_component::{validate_component, PathComponent, PathParser};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Longueur maximale dun chemin (4096 octets, POSIX PATH_MAX).
pub const PATH_MAX: usize = 4096;

// -- Stack de composants a taille fixe -----------------------------------------

/// Stack de composants de chemin, stockee sur la pile (pas de Vec).
/// Capacite : 64 composants max (chemin de 64 niveaux de profondeur).
pub struct ComponentStack {
    data: [Option<PathComponent>; 64],
    top: usize,
}

impl ComponentStack {
    pub fn new() -> Self {
        // SAFETY : Option<PathComponent> est Copy-capable ici (Clone au moins)
        ComponentStack {
            data: core::array::from_fn(|_| None),
            top: 0,
        }
    }

    pub fn push(&mut self, comp: PathComponent) -> ExofsResult<()> {
        if self.top >= 64 {
            return Err(ExofsError::PathTooLong);
        }
        self.data[self.top] = Some(comp);
        self.top = self.top.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        Ok(())
    }

    pub fn pop(&mut self) {
        if self.top > 0 {
            self.top -= 1;
            self.data[self.top] = None;
        }
    }

    pub fn len(&self) -> usize {
        self.top
    }
    pub fn is_empty(&self) -> bool {
        self.top == 0
    }

    pub fn as_slice(&self) -> &[Option<PathComponent>] {
        &self.data[..self.top]
    }
}

impl Default for ComponentStack {
    fn default() -> Self {
        Self::new()
    }
}

// -- canonicalize_path --------------------------------------------------------

/// Normalise un chemin brut en supprimant les redondances.
///
/// Operations effectuees :
/// 1. Collaps des slashes consecutifs ( => ).
/// 2. Suppression des composants  (repertoire courant).
/// 3. Resolution des composants  (repertoire parent).
/// 4. Verification de PATH_MAX.
///
/// Ecrit le resultat dans  et retourne la longueur ecrite.
///
/// # Regles spec
/// - **RECUR-01** : aucune recursion -- toutes les operations sont iteratives.
/// - **ARITH-02** : checked_add sur tous les offsets.
///
/// # Errors
/// -           si le resultat depasse PATH_MAX.
/// -  si un composant est invalide.
/// -         si calcul darithmetique deborde.
pub fn canonicalize_path(input: &[u8], buf: &mut [u8]) -> ExofsResult<usize> {
    if input.is_empty() {
        return Err(ExofsError::InvalidPathComponent);
    }
    if buf.len() < 2 {
        return Err(ExofsError::InvalidArgument);
    }

    let is_abs = input.first() == Some(&b'/');
    let mut stack = ComponentStack::new();

    // -- Phase 1 : parse iteratif et resolution . / .. (RECUR-01) --
    let mut parser = PathParser::new(input)?;
    loop {
        match parser.next_component()? {
            None => break,
            Some(comp) => {
                if comp.is_dot() {
                    // Ignorer . -- on reste dans le meme repertoire.
                } else if comp.is_dotdot() {
                    // .. -- remonter dun niveau si possible.
                    stack.pop();
                } else {
                    stack.push(comp)?;
                }
            }
        }
    }

    // -- Phase 2 : reconstruction iterative dans buf --
    let mut pos: usize = 0;

    if is_abs {
        if pos >= buf.len() {
            return Err(ExofsError::PathTooLong);
        }
        buf[pos] = b'/';
        pos = pos.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
    }

    for (i, slot) in stack.as_slice().iter().enumerate() {
        let comp = slot.as_ref().unwrap();
        if i > 0 || !is_abs {
            if pos >= buf.len() {
                return Err(ExofsError::PathTooLong);
            }
            if i > 0 {
                buf[pos] = b'/';
                pos = pos.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            }
        }
        let name = comp.as_bytes();
        let end = pos
            .checked_add(name.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if end > buf.len() {
            return Err(ExofsError::PathTooLong);
        }
        buf[pos..end].copy_from_slice(name);
        pos = end;
    }

    // Chemin vide apres normalisation = racine ou vide.
    if pos == 0 {
        if is_abs {
            // Deja ecrit le /
        } else {
            // Chemin relatif vide -- retourner .
            if buf.len() < 1 {
                return Err(ExofsError::PathTooLong);
            }
            buf[0] = b'.';
            pos = 1;
        }
    }

    if pos > PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }
    Ok(pos)
}

// -- canonicalize_to_vec ------------------------------------------------------

/// Version qui alloue le resultat dans un Vec.
///
/// # OOM-02 : try_reserve
pub fn canonicalize_to_vec(input: &[u8]) -> ExofsResult<Vec<u8>> {
    let mut buf = [0u8; PATH_MAX];
    let n = canonicalize_path(input, &mut buf)?;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    out.extend_from_slice(&buf[..n]);
    Ok(out)
}

// -- normalize_components ──────────────────────────────────────────────────────

/// Normalise une sequence de composants deja parses.
///
/// Resout les . et .. dans la sequence. Retourne un Vec de composants propres.
pub fn normalize_components(input: &[PathComponent]) -> ExofsResult<Vec<PathComponent>> {
    let mut stack: Vec<PathComponent> = Vec::new();
    for comp in input {
        if comp.is_dot() {
            // Ignorer.
        } else if comp.is_dotdot() {
            stack.pop();
        } else {
            stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            stack.push(comp.clone());
        }
    }
    Ok(stack)
}

// -- CanonicalPath ─────────────────────────────────────────────────────────────

/// Chemin canonique, garanti valide, stocke comme Vec<u8>.
#[derive(Clone, Debug)]
pub struct CanonicalPath {
    bytes: Vec<u8>,
}

impl CanonicalPath {
    /// Cree un CanonicalPath depuis un chemin brut.
    pub fn new(input: &[u8]) -> ExofsResult<Self> {
        let bytes = canonicalize_to_vec(input)?;
        Ok(CanonicalPath { bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Retourne le composant final (basename).
    pub fn basename(&self) -> Option<&[u8]> {
        let b = &self.bytes;
        if b == b"/" {
            return Some(b"/");
        }
        let start = b
            .iter()
            .rposition(|&c| c == b'/')
            .map(|p| p + 1)
            .unwrap_or(0);
        if start >= b.len() {
            return None;
        }
        Some(&b[start..])
    }

    /// Retourne le chemin sans le dernier composant (dirname).
    pub fn dirname(&self) -> &[u8] {
        let b = &self.bytes;
        match b.iter().rposition(|&c| c == b'/') {
            None => b".",
            Some(0) => b"/",
            Some(pos) => &b[..pos],
        }
    }

    /// Teste si ce chemin est un prefixe de .
    pub fn is_prefix_of(&self, other: &[u8]) -> bool {
        other.starts_with(&self.bytes)
            && (other.len() == self.bytes.len() || other.get(self.bytes.len()) == Some(&b'/'))
    }

    /// Joint ce chemin avec un composant supplementaire.
    pub fn join(&self, comp: &PathComponent) -> ExofsResult<Self> {
        let total = self
            .bytes
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(comp.len()))
            .ok_or(ExofsError::PathTooLong)?;
        if total > PATH_MAX {
            return Err(ExofsError::PathTooLong);
        }
        let mut b: Vec<u8> = Vec::new();
        b.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        b.extend_from_slice(&self.bytes);
        if self.bytes.last() != Some(&b'/') {
            b.push(b'/');
        }
        b.extend_from_slice(comp.as_bytes());
        Ok(CanonicalPath { bytes: b })
    }
}

// -- PathNormalizer : validateur strict ──────────────────────────────────────

/// Validateur de chemin canonique.
///
/// Vérifie les propriétés du chemin APRES canonicalisation :
/// - Commence par `/` (si absolu).
/// - Aucun `//`, `/.`, `/..`.
/// - Chaque composant valide selon les règles ExoFS.
pub struct PathNormalizer;

impl PathNormalizer {
    /// Vérifie qu un chemin est déjà canonique (rapide, sans allocation).
    ///
    /// Retourne `Ok(())` si le chemin est correct, une erreur sinon.
    pub fn verify(path: &[u8]) -> ExofsResult<()> {
        if path.is_empty() {
            return Err(ExofsError::InvalidPathComponent);
        }
        if path.len() > PATH_MAX {
            return Err(ExofsError::PathTooLong);
        }

        let mut prev_slash = false;
        let mut start: Option<usize> = None;

        for (i, &b) in path.iter().enumerate() {
            if b == b'/' {
                if prev_slash {
                    return Err(ExofsError::InvalidPathComponent);
                }
                if let Some(s) = start {
                    let comp = &path[s..i];
                    validate_component(comp)?;
                    start = None;
                }
                prev_slash = true;
            } else {
                if start.is_none() {
                    start = Some(i);
                }
                prev_slash = false;
            }
        }
        if let Some(s) = start {
            validate_component(&path[s..])?;
        }
        Ok(())
    }

    /// Vérifie ET retourne le chemin normalisé (canonique strict).
    pub fn verify_and_return(path: &[u8]) -> ExofsResult<CanonicalPath> {
        Self::verify(path)?;
        let bytes = {
            extern crate alloc;
            use alloc::vec::Vec;
            let mut v: Vec<u8> = Vec::new();
            v.try_reserve(path.len())
                .map_err(|_| ExofsError::NoMemory)?;
            v.extend_from_slice(path);
            v
        };
        Ok(CanonicalPath { bytes })
    }
}

// -- string_to_components : chemin => Vec de composants ─────────────────────

/// Découpe un chemin canonique en composants, sans résoudre . ni ..
///
/// Le chemin doit être déjà canonique (résultat de `canonicalize_path`).
pub fn split_canonical(path: &[u8]) -> ExofsResult<alloc::vec::Vec<PathComponent>> {
    use alloc::vec::Vec;
    let mut out: Vec<PathComponent> = Vec::new();
    let src = if path.first() == Some(&b'/') {
        &path[1..]
    } else {
        path
    };
    if src.is_empty() {
        return Ok(out);
    }

    let mut seg_start = 0;
    for (i, &b) in src.iter().enumerate() {
        if b == b'/' {
            let comp = validate_component(&src[seg_start..i])?;
            out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            out.push(comp);
            seg_start = i.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }
    }
    if seg_start < src.len() {
        let comp = validate_component(&src[seg_start..])?;
        out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        out.push(comp);
    }
    Ok(out)
}

/// Joint une liste de composants en un chemin absolu.
pub fn join_components(comps: &[PathComponent]) -> ExofsResult<alloc::vec::Vec<u8>> {
    use alloc::vec::Vec;
    let mut total: usize = 1; // leading '/'
    for c in comps {
        total = total
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?
            .checked_add(c.len())
            .ok_or(ExofsError::OffsetOverflow)?;
    }
    if total > PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    out.push(b'/');
    for (i, c) in comps.iter().enumerate() {
        if i > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(c.as_bytes());
    }
    Ok(out)
}

// -- Tests supplémentaires ────────────────────────────────────────────────────
#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn test_verify_ok() {
        PathNormalizer::verify(b"/home/user/file").unwrap();
    }
    #[test]
    fn test_verify_double_slash() {
        assert!(PathNormalizer::verify(b"//a").is_err());
    }
    #[test]
    fn test_split_canonical() {
        let comps = split_canonical(b"/a/b/c").unwrap();
        assert_eq!(comps.len(), 3);
        assert_eq!(comps[0].as_bytes(), b"a");
        assert_eq!(comps[2].as_bytes(), b"c");
    }
    #[test]
    fn test_join_components() {
        use super::super::path_component::validate_component;
        let comps = [
            validate_component(b"home").unwrap(),
            validate_component(b"user").unwrap(),
        ];
        let path = join_components(&comps).unwrap();
        assert_eq!(path, b"/home/user");
    }
    #[test]
    fn test_can_verify_and_return() {
        let p = PathNormalizer::verify_and_return(b"/proc/1/status").unwrap();
        assert_eq!(p.as_bytes(), b"/proc/1/status");
    }
    #[test]
    fn test_canonicalize_multi_dotdot() {
        let r = canonicalize_to_vec(b"/a/b/c/../../d").unwrap();
        assert_eq!(r, b"/a/d");
    }
}

// -- Tests --
#[cfg(test)]
mod tests {
    use super::*;

    fn canon(input: &[u8]) -> Vec<u8> {
        canonicalize_to_vec(input).unwrap()
    }

    #[test]
    fn test_absolute_simple() {
        assert_eq!(canon(b"/home/user"), b"/home/user");
    }
    #[test]
    fn test_trailing_slash() {
        assert_eq!(canon(b"/home/user/"), b"/home/user");
    }
    #[test]
    fn test_double_slash() {
        assert_eq!(canon(b"//home//user"), b"/home/user");
    }
    #[test]
    fn test_dot() {
        assert_eq!(canon(b"/a/./b"), b"/a/b");
    }
    #[test]
    fn test_dotdot() {
        assert_eq!(canon(b"/a/b/../c"), b"/a/c");
    }
    #[test]
    fn test_dotdot_beyond_root() {
        // /../../ = /
        assert_eq!(canon(b"/../.."), b"/");
    }
    #[test]
    fn test_relative() {
        assert_eq!(canon(b"a/b/c"), b"a/b/c");
    }
    #[test]
    fn test_relative_dotdot() {
        assert_eq!(canon(b"a/b/../c"), b"a/c");
    }
    #[test]
    fn test_root_only() {
        assert_eq!(canon(b"/"), b"/");
    }
    #[test]
    fn test_canonical_path_basename() {
        let c = CanonicalPath::new(b"/home/user/file.txt").unwrap();
        assert_eq!(c.basename().unwrap(), b"file.txt");
    }
    #[test]
    fn test_canonical_path_dirname() {
        let c = CanonicalPath::new(b"/home/user/file.txt").unwrap();
        assert_eq!(c.dirname(), b"/home/user");
    }
    #[test]
    fn test_is_prefix_of() {
        let c = CanonicalPath::new(b"/home").unwrap();
        assert!(c.is_prefix_of(b"/home/user"));
        assert!(!c.is_prefix_of(b"/homeother"));
    }
}
