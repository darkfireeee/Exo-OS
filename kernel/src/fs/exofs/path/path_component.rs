//! path_component.rs — Validated ExoFS path component.
//!
//! A [`PathComponent`] represents an individual segment of a filesystem
//! path (between two `/` separators). It is guaranteed to be:
//! - Non-empty.
//! - Length <= [`NAME_MAX`] (255 bytes).
//! - Free of null bytes (`\0`) and slashes (`/`).
//! - Valid UTF-8.
//!
//! Fixed stack storage — zero heap allocation for the component itself.
//! The parser [`PathParser`] splits a complete path into components in an
//! **iterative** manner (rule RECUR-01).
//!
//! # Applied spec rules
//! - **RECUR-01** : iterative parsing, never recursive.
//! - **OOM-02** : `try_reserve(1)` before any `Vec::push`.
//! - **ARITH-02** : `checked_add` on read offsets.

extern crate alloc;
use alloc::vec::Vec;

/// Maximum length of an ExoFS full path (in bytes, excluding null).
/// Conforms to POSIX PATH_MAX = 4096.
pub const PATH_MAX: usize = 4096;
use core::fmt;

use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Maximum length of a path component (255 bytes, POSIX NAME_MAX).
pub const NAME_MAX: usize = 255;

// ── PathComponent ─────────────────────────────────────────────────────────────

/// Validated path component — stored on the stack, never on the heap.
///
/// Size: `NAME_MAX + 3` bytes (storage + length u16).
/// Post-construction guarantees:
/// - Not empty.
/// - No `b'/'` nor `b'\0'`.
/// - Valid UTF-8.
/// - Length <= `NAME_MAX`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathComponent {
    bytes: [u8; NAME_MAX + 1],
    len: u16,
}

impl PathComponent {
    /// Returns raw bytes of the component.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// `true` if the component is empty (should never happen after validation).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` if the component is `.` (current directory).
    #[inline]
    pub fn is_dot(&self) -> bool {
        self.as_bytes() == b"."
    }

    /// `true` if the component is `..` (parent directory).
    #[inline]
    pub fn is_dotdot(&self) -> bool {
        self.as_bytes() == b".."
    }

    /// `true` if the component is `.` or `..`.
    #[inline]
    pub fn is_special(&self) -> bool {
        self.is_dot() || self.is_dotdot()
    }

    /// `&str` view of the component (guaranteed valid UTF-8 after validation).
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: `validate_component` guarantees valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(self.as_bytes()) }
    }

    /// FNV-1a hash of the component (used by `PathIndex` for sorting).
    pub fn fnv1a(&self) -> u64 {
        fnv1a_hash(self.as_bytes())
    }

    /// Case-insensitive ASCII comparison (for Windows-compatible systems).
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

    /// Constructs from pre-validated bytes (internal use).
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

    /// Returns a copy in a fixed-size array (useful for BTreeMap keys).
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

/// Validates and returns a path component from raw bytes.
///
/// # Validation rules (in order)
/// 1. Non-empty.
/// 2. Length <= `NAME_MAX`.
/// 3. No `b'/'` nor `b'\0'`.
/// 4. Valid UTF-8.
///
/// # Errors
/// - [`ExofsError::InvalidPathComponent`] if empty, contains `/`/`\0`, or invalid UTF-8.
/// - [`ExofsError::PathTooLong`] if length > `NAME_MAX`.
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

/// Validates a component without constructing the structure (fast check).
pub fn is_valid_component(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > NAME_MAX {
        return false;
    }
    if bytes.iter().any(|&b| b == b'/' || b == 0) {
        return false;
    }
    core::str::from_utf8(bytes).is_ok()
}

// ── PathParser — iterative splitting (RECUR-01) ────────────────────────────────

/// Iterative parser for a complete path.
///
/// Splits a path (e.g. `/home/user/file.txt`) into validated components.
/// Is **never** recursive (rule RECUR-01).
///
/// # Usage
/// ```ignore
/// let mut parser = PathParser::new(b"/home/user/file");
/// while let Some(comp) = parser.next_component()? { ... }
/// ```
pub struct PathParser<'a> {
    /// Path source.
    src: &'a [u8],
    /// Current position in `src`.
    pos: usize,
    /// Indicates whether the path was absolute (started with `/`).
    is_abs: bool,
    /// Remaining components to yield.
    done: bool,
}

impl<'a> PathParser<'a> {
    /// Creates a parser for the path `path`.
    ///
    /// # Errors
    /// - [`ExofsError::InvalidPathComponent`] if `path` is empty.
    /// - [`ExofsError::PathTooLong`] if `path` exceeds 4096 bytes.
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

    /// `true` if the path is absolute (starts with `/`).
    #[inline]
    pub fn is_absolute(&self) -> bool {
        self.is_abs
    }

    /// `true` if all components have been produced.
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.done
    }

    /// Returns the next validated component, or `None` at the end of the path.
    ///
    /// # Errors
    /// Propagates errors from [`validate_component`].
    pub fn next_component(&mut self) -> ExofsResult<Option<PathComponent>> {
        if self.done {
            return Ok(None);
        }

        // Skip consecutive slashes.
        while self.pos < self.src.len() && self.src[self.pos] == b'/' {
            self.pos = self.pos.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }

        if self.pos >= self.src.len() {
            self.done = true;
            return Ok(None);
        }

        // Find the end of the component.
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

    /// Collects all remaining components into a `Vec`.
    ///
    /// # OOM-02
    /// `try_reserve(1)` before each `push`.
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

    /// Returns the count of remaining slashes (estimate for depth).
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

// ── PathComponentBuf — vector of components ──────────────────────────────────

/// Vector of path components with manipulation methods.
///
/// All mutations respect **OOM-02** (`try_reserve(1)` before push).
#[derive(Clone, Debug, Default)]
pub struct PathComponentBuf {
    components: Vec<PathComponent>,
}

impl PathComponentBuf {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    /// Creates a pre-allocated buffer.
    ///
    /// # OOM-02
    pub fn with_capacity(cap: usize) -> ExofsResult<Self> {
        let mut v = Vec::new();
        v.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { components: v })
    }

    /// Parses a complete path and populates the buffer.
    pub fn from_path(path: &[u8]) -> ExofsResult<Self> {
        let mut parser = PathParser::new(path)?;
        let comps = parser.collect_all()?;
        Ok(Self { components: comps })
    }

    /// Adds a component (OOM-02).
    pub fn push(&mut self, comp: PathComponent) -> ExofsResult<()> {
        self.components
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.components.push(comp);
        Ok(())
    }

    /// Pops the last component (semantic `..`).
    pub fn pop(&mut self) {
        self.components.pop();
    }

    /// Number of components.
    #[inline]
    pub fn len(&self) -> usize {
        self.components.len()
    }
    /// `true` if no components.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
    /// Slice view.
    #[inline]
    pub fn as_slice(&self) -> &[PathComponent] {
        &self.components
    }
    /// Iterator.
    pub fn iter(&self) -> core::slice::Iter<'_, PathComponent> {
        self.components.iter()
    }

    /// Reconstructs the path as bytes (zero-allocation for small paths).
    ///
    /// Max 4096 bytes. Returns `Err(PathTooLong)` if exceeded.
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

    /// Applies `.` and `..` semantics: normalizes the buffer in place.
    pub fn normalize(&mut self) -> ExofsResult<()> {
        let mut result: Vec<PathComponent> = Vec::new();
        for comp in self.components.drain(..) {
            if comp.is_dot() {
                // Ignore `.`
            } else if comp.is_dotdot() {
                result.pop(); // Go up to parent.
            } else {
                result.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                result.push(comp);
            }
        }
        self.components = result;
        Ok(())
    }

    /// Returns the last component (tail / basename).
    pub fn last(&self) -> Option<&PathComponent> {
        self.components.last()
    }

    /// Returns all components except the last one (dirname).
    pub fn parent(&self) -> &[PathComponent] {
        let len = self.components.len();
        if len == 0 {
            &[]
        } else {
            &self.components[..len - 1]
        }
    }

    /// Extends with components from another buffer (OOM-02).
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

// ── Hash utilities ─────────────────────────────────────────────────────────────

/// Fast FNV-1a hash (kernel-safe, zero allocation).
#[inline]
pub fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Combined FNV-1a hash of two slices (e.g. namespace + component).
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

/// SipHash-2-4 with mount key (LAC-05/PATH-01/S-12).
///
/// Use instead of `fnv1a_hash` for any path indexing in a mounted volume —
/// `key` comes from `PathIndex`'s `mount_key`.
///
/// WARNING PATH-02: `key` must be generated randomly at mount time.
/// Never use `[0u8; 16]` in production.
#[inline]
pub fn siphash_keyed(key: &[u8; 16], data: &[u8]) -> u64 {
    use core::hash::Hasher;
    use siphasher::sip::SipHasher24;
    // SAFETY: pointer computed from slice whose length has been checked.
    let k0 = u64::from_le_bytes(unsafe { *(key[0..8].as_ptr() as *const [u8; 8]) });
    // SAFETY: pointer computed from slice whose length has been checked.
    let k1 = u64::from_le_bytes(unsafe { *(key[8..16].as_ptr() as *const [u8; 8]) });
    let mut h = SipHasher24::new_with_keys(k0, k1);
    h.write(data);
    h.finish()
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ok() {
        let c = validate_component(b"hello").test_unwrap();
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
        assert!(validate_component(b".").test_unwrap().is_dot());
        assert!(validate_component(b"..").test_unwrap().is_dotdot());
    }
    #[test]
    fn test_parser_absolute() {
        let mut p = PathParser::new(b"/home/user/file.txt").test_unwrap();
        assert!(p.is_absolute());
        assert_eq!(
            p.next_component().test_unwrap().test_unwrap().as_bytes(),
            b"home"
        );
        assert_eq!(
            p.next_component().test_unwrap().test_unwrap().as_bytes(),
            b"user"
        );
        assert_eq!(
            p.next_component().test_unwrap().test_unwrap().as_bytes(),
            b"file.txt"
        );
        assert!(p.next_component().test_unwrap().is_none());
    }
    #[test]
    fn test_parser_relative() {
        let mut p = PathParser::new(b"a/b").test_unwrap();
        assert!(!p.is_absolute());
        assert_eq!(
            p.next_component().test_unwrap().test_unwrap().as_bytes(),
            b"a"
        );
        assert_eq!(
            p.next_component().test_unwrap().test_unwrap().as_bytes(),
            b"b"
        );
    }
    #[test]
    fn test_parser_double_slash() {
        let mut p = PathParser::new(b"//a//b//").test_unwrap();
        let comps = p.collect_all().test_unwrap();
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[0].as_bytes(), b"a");
        assert_eq!(comps[1].as_bytes(), b"b");
    }
    #[test]
    fn test_component_buf_normalize() {
        let mut buf = PathComponentBuf::from_path(b"/a/b/../c/./d").test_unwrap();
        buf.normalize().test_unwrap();
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
        let a = validate_component(b"alpha").test_unwrap();
        let b = validate_component(b"beta").test_unwrap();
        assert!(a < b);
    }
    #[test]
    fn test_to_bytes_root() {
        let buf = PathComponentBuf::new();
        let bytes = buf.to_bytes().test_unwrap();
        assert_eq!(bytes, b"/");
    }
}
