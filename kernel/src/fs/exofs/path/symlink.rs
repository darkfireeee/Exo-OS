//! symlink.rs -- ExoFS symbolic links management.
//!
//! [SymlinkTarget] represents the target of a symbolic link (path in bytes).
//! [SymlinkStore] is a static table of targets (ring buffer of 128 entries).
//! Iterative symlink resolution respects SYMLINK_MAX_DEPTH (40 levels).
//!
//! ## Spec rules
//! - **RECUR-01** : iterative resolution, never recursive.
//! - **OOM-02**   : try_reserve(1) before push.
//! - **ARITH-02** : checked_add on all offset calculations.

extern crate alloc;
use alloc::vec::Vec;

use super::path_component::{PathComponent, PathParser};
use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::scheduler::sync::spinlock::SpinLock;

// -- Constants ---------------------------------------------------------------

/// Maximum depth of followed symlinks before loop detection.
pub const SYMLINK_MAX_DEPTH: usize = 40;
/// Maximum length of a symlink target.
pub const SYMLINK_TARGET_MAX: usize = 4096;
/// Store capacity (ring buffer, power of 2).
pub const STORE_SIZE: usize = 128;
const STORE_MASK: usize = STORE_SIZE - 1;

// -- SymlinkTarget ------------------------------------------------------------

/// Target of a symbolic link: raw path in bytes (max 4096).
///
/// Inline storage: no heap allocation per target.
#[derive(Clone)]
pub struct SymlinkTarget {
    bytes: [u8; SYMLINK_TARGET_MAX],
    len: u16,
    /// Symlink object OID.
    pub oid: ObjectId,
    /// `true` if target is an absolute path.
    pub is_absolute: bool,
}

impl SymlinkTarget {
    /// Creates a symlink target from a byte slice.
    pub fn new(raw: &[u8], oid: ObjectId) -> ExofsResult<Self> {
        if raw.is_empty() {
            return Err(ExofsError::InvalidPathComponent);
        }
        if raw.len() > SYMLINK_TARGET_MAX {
            return Err(ExofsError::PathTooLong);
        }
        if raw.contains(&0u8) {
            return Err(ExofsError::InvalidPathComponent);
        }
        let is_absolute = raw.first() == Some(&b'/');
        let mut storage = [0u8; SYMLINK_TARGET_MAX];
        storage[..raw.len()].copy_from_slice(raw);
        Ok(SymlinkTarget {
            bytes: storage,
            len: raw.len() as u16,
            oid,
            is_absolute,
        })
    }

    /// Returns target bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
    /// Target length.
    pub fn len(&self) -> usize {
        self.len as usize
    }
    /// `true` if target is empty (should not happen).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl core::fmt::Debug for SymlinkTarget {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "SymlinkTarget({:?})",
            core::str::from_utf8(self.as_bytes()).unwrap_or("?")
        )
    }
}

// -- SymlinkStoreEntry --------------------------------------------------------

struct SymlinkStoreEntry {
    oid: ObjectId,
    target: [u8; SYMLINK_TARGET_MAX],
    len: u16,
    valid: bool,
}

impl SymlinkStoreEntry {
    const fn empty() -> Self {
        SymlinkStoreEntry {
            oid: ObjectId::INVALID,
            target: [0u8; SYMLINK_TARGET_MAX],
            len: 0,
            valid: false,
        }
    }

    fn target_bytes(&self) -> &[u8] {
        &self.target[..self.len as usize]
    }
}

// -- SymlinkStoreInner --------------------------------------------------------

struct SymlinkStoreInner {
    entries: [SymlinkStoreEntry; STORE_SIZE],
    head: usize,
    count: usize,
}

impl SymlinkStoreInner {
    const fn new() -> Self {
        SymlinkStoreInner {
            entries: [const { SymlinkStoreEntry::empty() }; STORE_SIZE],
            head: 0,
            count: 0,
        }
    }

    fn store(&mut self, oid: &ObjectId, target: &[u8]) -> ExofsResult<()> {
        if target.len() > SYMLINK_TARGET_MAX {
            return Err(ExofsError::PathTooLong);
        }
        let slot = self.head & STORE_MASK;
        let e = &mut self.entries[slot];
        e.oid = oid.clone();
        e.len = target.len() as u16;
        e.target[..target.len()].copy_from_slice(target);
        e.valid = true;
        self.head = self.head.wrapping_add(1);
        if self.count < STORE_SIZE {
            self.count = self.count.saturating_add(1);
        }
        Ok(())
    }

    fn lookup(&self, oid: &ObjectId) -> Option<&[u8]> {
        for e in &self.entries {
            if e.valid && e.oid.as_bytes() == oid.as_bytes() {
                return Some(e.target_bytes());
            }
        }
        None
    }

    fn invalidate(&mut self, oid: &ObjectId) {
        for e in &mut self.entries {
            if e.valid && e.oid.as_bytes() == oid.as_bytes() {
                e.valid = false;
            }
        }
    }

    fn flush(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
        self.count = 0;
    }

    fn count(&self) -> usize {
        self.count
    }
}

// -- SymlinkStore -------------------------------------------------------------

/// Symlink target store (thread-safe, ring buffer).
pub struct SymlinkStore {
    inner: SpinLock<SymlinkStoreInner>,
}

impl SymlinkStore {
    pub const fn new_const() -> Self {
        SymlinkStore {
            inner: SpinLock::new(SymlinkStoreInner::new()),
        }
    }

    /// Stores a symlink target.
    pub fn store(&self, oid: &ObjectId, target: &[u8]) -> ExofsResult<()> {
        self.inner.lock().store(oid, target)
    }

    /// Looks up a symlink OID target. Returns a copy.
    pub fn lookup(&self, oid: &ObjectId) -> Option<Vec<u8>> {
        let g = self.inner.lock();
        g.lookup(oid).map(|b| {
            let mut v = Vec::new();
            if v.try_reserve(b.len()).is_ok() {
                v.extend_from_slice(b);
            }
            v
        })
    }

    /// Invalidates entry for an OID.
    pub fn invalidate(&self, oid: &ObjectId) {
        self.inner.lock().invalidate(oid);
    }
    /// Flushes the store.
    pub fn flush(&self) {
        self.inner.lock().flush();
    }
    /// Number of valid entries.
    pub fn count(&self) -> usize {
        self.inner.lock().count()
    }
}

/// Global symlink store.
pub static SYMLINK_STORE: SymlinkStore = SymlinkStore::new_const();

// -- SymlinkChain : iterative chain resolution (RECUR-01) ─────────────────

/// Result of resolving a chain of symlinks.
#[derive(Debug)]
pub struct SymlinkResolution {
    /// Resolved components.
    pub components: Vec<PathComponent>,
    /// Number of indirection levels followed.
    pub depth: usize,
    /// `true` if final path is absolute.
    pub is_absolute: bool,
}

/// Iteratively resolves a chain of symlinks.
///
/// - `initial_target`: first symlink target (raw bytes).
/// - `max_depth`     : SYMLINK_MAX_DEPTH limit.
/// - `resolver`      : callback to obtain the target of a component.
///
/// # Spec rules
/// - **RECUR-01** : iterative loop, no recursion.
/// - **OOM-02**   : try_reserve(1) before push.
///
/// # Errors
/// - `TooManySymlinks` if depth > max_depth.
/// - `InvalidPathComponent` / `PathTooLong` if invalid path.
pub fn resolve_symlink_chain<F>(
    initial_target: &[u8],
    max_depth: usize,
    resolver: F,
) -> ExofsResult<SymlinkResolution>
where
    F: Fn(&[u8]) -> Option<Vec<u8>>,
{
    let mut depth: usize = 0;
    let mut current: Vec<u8>;
    let mut components: Vec<PathComponent> = Vec::new();
    let mut is_absolute: bool;

    // Initial copy.
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(initial_target.len())
        .map_err(|_| ExofsError::NoMemory)?;
    buf.extend_from_slice(initial_target);
    current = buf;

    // Iterative resolution loop.
    loop {
        if depth > max_depth {
            return Err(ExofsError::TooManySymlinks);
        }

        is_absolute = current.first() == Some(&b'/');
        let mut parser = PathParser::new(&current)?;
        components.clear();

        let mut found_symlink = false;
        loop {
            match parser.next_component()? {
                None => break,
                Some(comp) => {
                    // Check if this component is itself a symlink.
                    if let Some(next_target) = resolver(comp.as_bytes()) {
                        // Substitute.
                        current = next_target;
                        depth = depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
                        if depth > max_depth {
                            return Err(ExofsError::TooManySymlinks);
                        }
                        found_symlink = true;
                        break;
                    } else {
                        components
                            .try_reserve(1)
                            .map_err(|_| ExofsError::NoMemory)?;
                        components.push(comp);
                    }
                }
            }
        }

        if !found_symlink {
            break;
        }
    }

    Ok(SymlinkResolution {
        components,
        depth,
        is_absolute,
    })
}

// -- Utilities ──────────────────────────────────────────────────────────────

/// Checks whether `raw` is a valid symlink target.
pub fn is_valid_symlink_target(raw: &[u8]) -> bool {
    !raw.is_empty() && raw.len() <= SYMLINK_TARGET_MAX && !raw.contains(&0u8)
}

/// Registers a symlink target in the global store.
pub fn register_symlink(oid: &ObjectId, target: &[u8]) -> ExofsResult<()> {
    SYMLINK_STORE.store(oid, target)
}

/// Invalidates a symlink in the global store.
pub fn invalidate_symlink(oid: &ObjectId) {
    SYMLINK_STORE.invalidate(oid);
}

// -- SymlinkResolveContext : resolution context ───────────────────────────

/// Context for resolving a chain of symlinks.
///
/// Maintains complete state for multi-step iterative resolution.
pub struct SymlinkResolveContext {
    /// Current working path.
    current_path: Vec<u8>,
    /// Reached resolution depth.
    pub depth: usize,
    /// Depth limit.
    pub limit: usize,
}

impl SymlinkResolveContext {
    /// Creates a context for initial path.
    pub fn new(path: &[u8], limit: usize) -> ExofsResult<Self> {
        if path.len() > SYMLINK_TARGET_MAX {
            return Err(ExofsError::PathTooLong);
        }
        let mut current_path: Vec<u8> = Vec::new();
        current_path
            .try_reserve(path.len())
            .map_err(|_| ExofsError::NoMemory)?;
        current_path.extend_from_slice(path);
        Ok(SymlinkResolveContext {
            current_path,
            depth: 0,
            limit,
        })
    }

    /// Follows a symlink: updates current path.
    pub fn follow(&mut self, target: &[u8]) -> ExofsResult<()> {
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        if self.depth > self.limit {
            return Err(ExofsError::TooManySymlinks);
        }
        if target.len() > SYMLINK_TARGET_MAX {
            return Err(ExofsError::PathTooLong);
        }
        self.current_path.clear();
        self.current_path
            .try_reserve(target.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.current_path.extend_from_slice(target);
        Ok(())
    }

    /// Current path.
    pub fn current(&self) -> &[u8] {
        &self.current_path
    }

    /// `true` if depth limit is reached.
    pub fn is_exhausted(&self) -> bool {
        self.depth >= self.limit
    }
}

// -- SymlinkMetadata ----------------------------------------------------------

/// Metadata attached to a symbolic link.
#[derive(Clone, Debug)]
pub struct SymlinkMetadata {
    pub oid: ObjectId,
    pub target_len: usize,
    pub is_absolute: bool,
    pub creation_tick: u64,
}

impl SymlinkMetadata {
    /// Creates metadata from a target.
    pub fn from_target(oid: ObjectId, target: &[u8]) -> Self {
        SymlinkMetadata {
            oid,
            target_len: target.len(),
            is_absolute: target.first() == Some(&b'/'),
            creation_tick: crate::arch::time::read_ticks(),
        }
    }
}

// -- Structural verification functions -----------------------------------

/// Verifies the structure of a symlink target.
///
/// - Not empty.
/// - Length <= SYMLINK_TARGET_MAX.
/// - No NUL.
/// - Valid UTF-8.
pub fn validate_symlink_target(raw: &[u8]) -> ExofsResult<()> {
    if raw.is_empty() {
        return Err(ExofsError::InvalidPathComponent);
    }
    if raw.len() > SYMLINK_TARGET_MAX {
        return Err(ExofsError::PathTooLong);
    }
    if raw.contains(&0u8) {
        return Err(ExofsError::InvalidPathComponent);
    }
    core::str::from_utf8(raw).map_err(|_| ExofsError::InvalidPathComponent)?;
    Ok(())
}

/// Verifies and returns a `SymlinkTarget`.
pub fn checked_symlink_target(raw: &[u8], oid: ObjectId) -> ExofsResult<SymlinkTarget> {
    validate_symlink_target(raw)?;
    SymlinkTarget::new(raw, oid)
}

// -- Additional tests ---------------------------------------------------
#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod extra_tests {
    use super::*;

    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    #[test]
    fn test_resolve_context_follow() {
        let mut ctx = SymlinkResolveContext::new(b"/a/b", 5).test_unwrap();
        ctx.follow(b"/c/d").test_unwrap();
        assert_eq!(ctx.current(), b"/c/d");
        assert_eq!(ctx.depth, 1);
    }

    #[test]
    fn test_resolve_context_exhaustion() {
        let mut ctx = SymlinkResolveContext::new(b"/loop", 2).test_unwrap();
        ctx.follow(b"/loop").test_unwrap();
        ctx.follow(b"/loop").test_unwrap();
        assert!(ctx.is_exhausted());
        assert!(ctx.follow(b"/loop").is_err());
    }

    #[test]
    fn test_metadata() {
        let oid = fake_oid(9);
        let m = SymlinkMetadata::from_target(oid.clone(), b"/var/run");
        assert!(m.is_absolute);
        assert_eq!(m.target_len, 8);
    }

    #[test]
    fn test_validate_target_ok() {
        validate_symlink_target(b"/valid").test_unwrap();
    }

    #[test]
    fn test_validate_target_nul() {
        let bad = b"path\x00nul";
        assert!(validate_symlink_target(bad).is_err());
    }

    #[test]
    fn test_checked_target() {
        let oid = fake_oid(10);
        let t = checked_symlink_target(b"/usr/local", oid).test_unwrap();
        assert_eq!(t.as_bytes(), b"/usr/local");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    #[test]
    fn test_symlink_target_new() {
        let t = SymlinkTarget::new(b"/home/user", fake_oid(1)).test_unwrap();
        assert_eq!(t.as_bytes(), b"/home/user");
        assert!(t.is_absolute);
    }
    #[test]
    fn test_symlink_target_relative() {
        let t = SymlinkTarget::new(b"../other", fake_oid(2)).test_unwrap();
        assert!(!t.is_absolute);
    }
    #[test]
    fn test_symlink_target_empty() {
        assert!(matches!(
            SymlinkTarget::new(b"", fake_oid(3)),
            Err(ExofsError::InvalidPathComponent)
        ));
    }
    #[test]
    fn test_store_lookup() {
        let store = SymlinkStore::new_const();
        let oid = fake_oid(4);
        store.store(&oid, b"/var/run").test_unwrap();
        let res = store.lookup(&oid).test_unwrap();
        assert_eq!(res, b"/var/run");
    }
    #[test]
    fn test_store_invalidate() {
        let store = SymlinkStore::new_const();
        let oid = fake_oid(5);
        store.store(&oid, b"/tmp").test_unwrap();
        store.invalidate(&oid);
        assert!(store.lookup(&oid).is_none());
    }
    #[test]
    fn test_depth_limit() {
        // Resolver that always loops.
        let result = resolve_symlink_chain(b"/loop", 40, |_| Some(b"/loop".to_vec()));
        assert!(matches!(result, Err(ExofsError::TooManySymlinks)));
    }
    #[test]
    fn test_resolve_no_symlink() {
        let res = resolve_symlink_chain(
            b"/home/user/file",
            40,
            |_| None, // No component is a symlink.
        )
        .test_unwrap();
        assert_eq!(res.depth, 0);
        assert!(res.is_absolute);
    }
    #[test]
    fn test_is_valid_target() {
        assert!(is_valid_symlink_target(b"/valid/path"));
        assert!(!is_valid_symlink_target(b""));
        assert!(!is_valid_symlink_target(b"path\0with\0nul"));
    }
}
