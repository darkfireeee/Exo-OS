//! resolver.rs — Iterative ExoFS path resolution
//!
//! Critical applied rules:
//!  - RECUR-01 : zero recursion — explicit `while` loop
//!  - PATH-07  : ResolveContext allocated on the heap (Vec<PathComponent>)
//!  - OOM-02   : try_reserve before any push
//!  - ARITH-02 : checked arithmetic

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::path::canonicalize::canonicalize_to_vec;
use crate::fs::exofs::path::mount_point::MOUNT_TABLE;
use crate::fs::exofs::path::path_cache::{cached_lookup, CacheLookup, PATH_CACHE};
use crate::fs::exofs::path::path_component::{PathComponent, PathParser, PATH_MAX};
use crate::fs::exofs::path::symlink::{SYMLINK_MAX_DEPTH, SYMLINK_STORE};

// ─────────────────────────────────────────────────────────────────────────────
// Trait PathResolver
// ─────────────────────────────────────────────────────────────────────────────

/// Interface that any filesystem backend must implement
/// to enable path resolution.
pub trait PathResolver {
    /// Searches for `name` in `dir_oid`.
    ///
    /// Returns `(oid, is_symlink)` or `None` if absent.
    fn lookup_in_dir(
        &self,
        dir_oid: &ObjectId,
        name: &PathComponent,
    ) -> ExofsResult<Option<(ObjectId, bool)>>;

    /// Reads target of a symbolic link.
    fn symlink_target(&self, oid: &ObjectId) -> ExofsResult<Vec<u8>>;

    /// Root OID of the filesystem.
    fn root_oid(&self) -> ObjectId;
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveContext — heap-allocated state (PATH-07)
// ─────────────────────────────────────────────────────────────────────────────

/// Context of an in-progress resolution.
///
/// Allocated on the heap to avoid `[u8; PATH_MAX]` on the stack (PATH-07).
pub struct ResolveContext {
    /// Current OID during traversal.
    pub current_oid: ObjectId,
    /// Remaining components to resolve (LIFO order — popped from end).
    pub remaining: Vec<PathComponent>,
    /// Symlink resolution depth.
    pub symlink_depth: usize,
    /// Original canonical path (for cache).
    pub canonical_path: Vec<u8>,
    /// Resolution flags.
    pub flags: ResolveFlags,
}

impl ResolveContext {
    /// Creates a context from a canonical path.
    pub fn new(root: ObjectId, canonical: Vec<u8>, flags: ResolveFlags) -> ExofsResult<Self> {
        let mut remaining: Vec<PathComponent> = Vec::new();
        let mut parser = PathParser::new(&canonical)?;
        while let Some(c) = parser.next_component()? {
            if c.as_bytes() == b"." {
                continue;
            }
            if c.as_bytes() == b".." {
                remaining.pop();
                continue;
            }
            remaining.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            remaining.push(c);
        }
        remaining.reverse(); // pop() will yield next component
        Ok(ResolveContext {
            current_oid: root,
            remaining,
            symlink_depth: 0,
            canonical_path: canonical,
            flags,
        })
    }

    /// `true` if all components have been consumed.
    #[inline]
    pub fn is_done(&self) -> bool {
        self.remaining.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveFlags
// ─────────────────────────────────────────────────────────────────────────────

/// Resolution options.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResolveFlags {
    /// Do not follow terminal symlink.
    pub no_follow_last: bool,
    /// Do not cross mount points.
    pub no_cross_mount: bool,
    /// Use cache.
    pub use_cache: bool,
    /// Cache the result.
    pub cache_result: bool,
}

impl ResolveFlags {
    /// Default flags (cache enabled).
    pub const fn default_flags() -> Self {
        ResolveFlags {
            no_follow_last: false,
            no_cross_mount: false,
            use_cache: true,
            cache_result: true,
        }
    }

    /// Resolution without following final symlink (for `lstat`).
    pub const fn lstat_flags() -> Self {
        ResolveFlags {
            no_follow_last: true,
            no_cross_mount: false,
            use_cache: true,
            cache_result: true,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveResult
// ─────────────────────────────────────────────────────────────────────────────

/// Enriched resolution result.
#[derive(Debug)]
pub struct ResolveResult {
    /// OID of resolved object.
    pub oid: ObjectId,
    /// `true` if terminal object is a symlink (in `no_follow_last` mode).
    pub is_symlink: bool,
    /// Number of symlinks traversed.
    pub symlink_hops: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// resolve_path — main entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves `path` from the root of `resolver`.
///
/// This function is the reference implementation:
///   1. Canonicalizes path.
///   2. Consults cache (if enabled).
///   3. Descends component by component (RECUR-01).
///   4. Traverses symlinks (iterative).
///   5. Traverses mount points.
///   6. Caches final result (if enabled).
pub fn resolve_path<R: PathResolver>(path: &[u8], resolver: &R) -> ExofsResult<ObjectId> {
    let result = resolve_path_full(path, resolver, ResolveFlags::default_flags())?;
    Ok(result.oid)
}

/// Resolves with enriched result and explicit flags.
pub fn resolve_path_full<R: PathResolver>(
    path: &[u8],
    resolver: &R,
    flags: ResolveFlags,
) -> ExofsResult<ResolveResult> {
    if path.len() > PATH_MAX {
        return Err(ExofsError::PathTooLong);
    }

    // 1. Canonicalization
    let canonical = canonicalize_to_vec(path)?;

    // 2. Cache lookup
    if flags.use_cache {
        if let CacheLookup::Hit(oid) = cached_lookup(&canonical) {
            return Ok(ResolveResult {
                oid,
                is_symlink: false,
                symlink_hops: 0,
            });
        }
    }

    // 3. Prepare context
    let root = resolver.root_oid();
    let mut ctx = ResolveContext::new(root, canonical.clone(), flags)?;

    // 4. Iterative descent (RECUR-01 — zero recursion)
    let mut iters: usize = 0;
    let max_iters: usize = 8192;

    loop {
        if iters >= max_iters {
            return Err(ExofsError::InternalError);
        }
        iters = iters.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;

        if ctx.is_done() {
            // 5. Mount points
            if !flags.no_cross_mount {
                if let Some(mounted) = MOUNT_TABLE.lookup_mount(&ctx.current_oid) {
                    ctx.current_oid = mounted;
                }
            }
            break;
        }

        let comp = ctx.remaining.pop().ok_or(ExofsError::InternalError)?;
        let is_last = ctx.remaining.is_empty();

        // Lookup in current directory
        let found = resolver.lookup_in_dir(&ctx.current_oid, &comp)?;
        let (oid, is_symlink) = match found {
            None => return Err(ExofsError::ObjectNotFound),
            Some(entry) => entry,
        };

        // Mount point traversal
        let effective_oid = if !flags.no_cross_mount {
            MOUNT_TABLE
                .lookup_mount(&oid)
                .unwrap_or_else(|| oid.clone())
        } else {
            oid.clone()
        };

        if is_symlink && !(is_last && flags.no_follow_last) {
            // Symlink resolution
            let new_depth = ctx
                .symlink_depth
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
            if new_depth > SYMLINK_MAX_DEPTH {
                return Err(ExofsError::TooManySymlinks);
            }
            ctx.symlink_depth = new_depth;

            // Check local store first
            let raw_target = if let Some(t) = SYMLINK_STORE.lookup(&oid) {
                t
            } else {
                resolver.symlink_target(&oid)?
            };

            if raw_target.len() > PATH_MAX {
                return Err(ExofsError::PathTooLong);
            }

            // Prepare new component sequence from target
            let target_canonical = canonicalize_to_vec(&raw_target)?;
            let new_root = if raw_target.first() == Some(&b'/') {
                resolver.root_oid()
            } else {
                ctx.current_oid.clone()
            };

            // Rebuild remaining component queue from target
            let mut new_remaining: Vec<PathComponent> = Vec::new();
            let mut parser = PathParser::new(&target_canonical)?;
            while let Some(c) = parser.next_component()? {
                if c.as_bytes() == b"." {
                    continue;
                }
                if c.as_bytes() == b".." {
                    new_remaining.pop();
                    continue;
                }
                new_remaining
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                new_remaining.push(c);
            }
            // Add remaining components of original path
            // (already in LIFO order in ctx.remaining)
            while let Some(old_comp) = ctx.remaining.pop() {
                new_remaining
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                new_remaining.push(old_comp);
            }
            new_remaining.reverse();

            ctx.current_oid = new_root;
            ctx.remaining = new_remaining;
            continue;
        }

        ctx.current_oid = effective_oid;
    }

    let result_oid = ctx.current_oid.clone();

    // 6. Cache insertion
    if flags.cache_result {
        PATH_CACHE.insert_path(&canonical, result_oid.clone());
    }

    Ok(ResolveResult {
        oid: result_oid,
        is_symlink: false,
        symlink_hops: ctx.symlink_depth,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves path without following terminal symlink (`lstat` semantics).
pub fn resolve_no_follow<R: PathResolver>(path: &[u8], resolver: &R) -> ExofsResult<ResolveResult> {
    resolve_path_full(path, resolver, ResolveFlags::lstat_flags())
}

/// Resolves a path and verifies whether it exists.
pub fn path_exists<R: PathResolver>(path: &[u8], resolver: &R) -> bool {
    resolve_path(path, resolver).is_ok()
}

/// Resolves parent directory.
pub fn resolve_parent<R: PathResolver>(path: &[u8], resolver: &R) -> ExofsResult<ObjectId> {
    let canonical = canonicalize_to_vec(path)?;
    let slash = canonical.iter().rposition(|&b| b == b'/');
    let parent = match slash {
        None | Some(0) => b"/" as &[u8],
        Some(p) => &canonical[..p],
    };
    resolve_path(parent, resolver)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    struct MockResolver;
    impl PathResolver for MockResolver {
        fn lookup_in_dir(
            &self,
            _dir: &ObjectId,
            comp: &PathComponent,
        ) -> ExofsResult<Option<(ObjectId, bool)>> {
            match comp.as_bytes() {
                b"usr" => Ok(Some((oid(10), false))),
                b"bin" => Ok(Some((oid(11), false))),
                b"lnk" => Ok(Some((oid(20), true))),
                _ => Ok(None),
            }
        }
        fn symlink_target(&self, _oid: &ObjectId) -> ExofsResult<Vec<u8>> {
            let mut v = Vec::new();
            v.extend_from_slice(b"/usr/bin");
            Ok(v)
        }
        fn root_oid(&self) -> ObjectId {
            oid(1)
        }
    }

    #[test]
    fn test_resolve_simple() {
        let r = MockResolver;
        let res = resolve_path(b"/usr/bin", &r).test_unwrap();
        assert_eq!(res.0[0], 11);
    }

    #[test]
    fn test_resolve_not_found() {
        let r = MockResolver;
        assert!(resolve_path(b"/usr/missing", &r).is_err());
    }

    #[test]
    fn test_resolve_symlink() {
        let r = MockResolver;
        // /lnk → /usr/bin
        let res = resolve_path(b"/lnk", &r).test_unwrap();
        assert_eq!(res.0[0], 11);
    }

    #[test]
    fn test_resolve_no_follow() {
        let r = MockResolver;
        let res = resolve_no_follow(b"/lnk", &r).test_unwrap();
        // With no_follow_last, we must stay on the symlink itself
        assert_eq!(res.oid.0[0], 20);
    }

    #[test]
    fn test_path_exists() {
        let r = MockResolver;
        assert!(path_exists(b"/usr", &r));
        assert!(!path_exists(b"/nonexistent", &r));
    }

    #[test]
    fn test_resolve_parent() {
        let r = MockResolver;
        let p = resolve_parent(b"/usr/bin", &r).test_unwrap();
        assert_eq!(p.0[0], 10);
    }
}
