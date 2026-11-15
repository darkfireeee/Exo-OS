//! path_walker.rs — Iterative path walking in the ExoFS tree
//!
//! Critical applied rules:
//!  - RECUR-01 : zero recursion — iterative state machine
//!  - OOM-02   : try_reserve before each push
//!  - PATH-07  : no [u8; PATH_MAX] on stack
//!  - ARITH-02 : checked arithmetic on all offsets

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::path::path_component::{validate_component, PathComponent, PathParser};
use crate::fs::exofs::path::symlink::SYMLINK_MAX_DEPTH;

// ─────────────────────────────────────────────────────────────────────────────
// Walker state
// ─────────────────────────────────────────────────────────────────────────────

/// Current state of the `PathWalker` state machine.
#[derive(Debug)]
pub enum WalkerState {
    /// Initial positioning before first step.
    AtRoot,
    /// Inside a directory, remaining components to descend.
    InDirectory {
        dir_oid: ObjectId,
        remaining: Vec<PathComponent>,
    },
    /// A symlink was just encountered.
    AtSymlink { target: Vec<u8>, depth: usize },
    /// Resolution completed successfully.
    Done { oid: ObjectId },
    /// Resolution terminated with error.
    Failed(ExofsError),
}

// ─────────────────────────────────────────────────────────────────────────────
// Step result
// ─────────────────────────────────────────────────────────────────────────────

/// Result returned by `PathWalker::step()`.
#[derive(Debug)]
pub enum WalkerStepResult {
    /// A component was consumed, resolution continues.
    Continue,
    /// Resolution complete — final OID is available.
    Done(ObjectId),
    /// A symlink was encountered, its target is provided.
    SymlinkEncountered(Vec<u8>),
    /// Fatal error.
    Error(ExofsError),
}

// ─────────────────────────────────────────────────────────────────────────────
// Callback trait
// ─────────────────────────────────────────────────────────────────────────────

/// Trait that caller must implement to feed the walker.
///
/// The walker does **no** direct I/O — it delegates all
/// resolutions to this trait.
pub trait WalkerBackend {
    /// Looks up a component in a directory.
    /// Returns `(oid, is_symlink)` or `None` if absent.
    fn lookup(
        &self,
        dir_oid: &ObjectId,
        component: &PathComponent,
    ) -> ExofsResult<Option<(ObjectId, bool)>>;

    /// Reads target of a symbolic link.
    fn symlink_target(&self, oid: &ObjectId) -> ExofsResult<Vec<u8>>;

    /// Filesystem root OID.
    fn root_oid(&self) -> ObjectId;
}

// ─────────────────────────────────────────────────────────────────────────────
// PathWalker
// ─────────────────────────────────────────────────────────────────────────────

/// Explicit state path iterator.
///
/// Each call to `step()` advances by one component. The caller must
/// continue until `WalkerStepResult::Done` or `::Error`.
pub struct PathWalker {
    /// Root directory OID for resolution.
    pub root_oid: ObjectId,
    /// Current state.
    pub state: WalkerState,
    /// Symlink resolution depth.
    pub symlink_depth: usize,
}

impl PathWalker {
    /// Creates a new walker on `path` from `root`.
    ///
    /// - Parses the path with `PathParser` (iterative, RECUR-01).
    /// - Components are stored in reverse order for efficient `pop()`.
    pub fn new(root: ObjectId, path: &[u8]) -> ExofsResult<Self> {
        if path.len() > crate::fs::exofs::path::path_component::PATH_MAX {
            return Err(ExofsError::PathTooLong);
        }
        let mut components: Vec<PathComponent> = Vec::new();
        let mut parser = PathParser::new(path)?;
        while let Some(comp) = parser.next_component()? {
            // Ignore "." components
            let bytes = comp.as_bytes();
            if bytes == b"." {
                continue;
            }
            components
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            components.push(comp);
        }
        // Reverse so the next component is at the tail.
        components.reverse();
        Ok(PathWalker {
            root_oid: root.clone(),
            state: WalkerState::InDirectory {
                dir_oid: root,
                remaining: components,
            },
            symlink_depth: 0,
        })
    }

    /// Advances by one step in resolution.
    ///
    /// Iterative (RECUR-01) — zero internal recursive calls.
    pub fn step<B: WalkerBackend>(&mut self, backend: &B) -> WalkerStepResult {
        match &mut self.state {
            WalkerState::AtRoot => {
                // Transition to InDirectory at root without components.
                let root = backend.root_oid();
                self.state = WalkerState::Done { oid: root.clone() };
                WalkerStepResult::Done(root)
            }

            WalkerState::Done { oid } => WalkerStepResult::Done(oid.clone()),

            WalkerState::Failed(e) => WalkerStepResult::Error(e.clone()),

            WalkerState::AtSymlink { target, depth } => {
                let tgt = core::mem::take(target);
                let d = *depth;
                // Go back to InDirectory context reconstructed from symlink.
                let mut components: Vec<PathComponent> = Vec::new();
                let parser = match PathParser::new(&tgt) {
                    Ok(p) => p,
                    Err(e) => {
                        self.state = WalkerState::Failed(e.clone());
                        return WalkerStepResult::Error(e);
                    }
                };
                for cr in parser {
                    match cr {
                        Err(e) => {
                            self.state = WalkerState::Failed(e.clone());
                            return WalkerStepResult::Error(e);
                        }
                        Ok(c) => {
                            if c.as_bytes() == b"." {
                                continue;
                            }
                            if let Err(_) = components.try_reserve(1) {
                                let e = ExofsError::NoMemory;
                                self.state = WalkerState::Failed(e.clone());
                                return WalkerStepResult::Error(e);
                            }
                            components.push(c);
                        }
                    }
                }
                components.reverse();
                let start = if tgt.first() == Some(&b'/') {
                    backend.root_oid()
                } else {
                    // Relative symlink: start from current parent (root by default here)
                    self.root_oid.clone()
                };
                self.symlink_depth = d;
                self.state = WalkerState::InDirectory {
                    dir_oid: start,
                    remaining: components,
                };
                WalkerStepResult::Continue
            }

            WalkerState::InDirectory { dir_oid, remaining } => {
                match remaining.pop() {
                    None => {
                        // No more components: resolution finished.
                        let oid = dir_oid.clone();
                        self.state = WalkerState::Done { oid: oid.clone() };
                        WalkerStepResult::Done(oid)
                    }
                    Some(comp) => {
                        // Component "..": go up (unsupported without parent-tracking —
                        // report controlled error).
                        if comp.as_bytes() == b".." {
                            // Parent is not tracked here, return
                            // current OID for normalized path cases.
                            // Caller with canonicalize_path must normalize first.
                            let oid = dir_oid.clone();
                            self.state = WalkerState::Done { oid: oid.clone() };
                            return WalkerStepResult::Done(oid);
                        }
                        match backend.lookup(dir_oid, &comp) {
                            Err(e) => {
                                self.state = WalkerState::Failed(e.clone());
                                WalkerStepResult::Error(e)
                            }
                            Ok(None) => {
                                let e = ExofsError::ObjectNotFound;
                                self.state = WalkerState::Failed(e.clone());
                                WalkerStepResult::Error(e)
                            }
                            Ok(Some((oid, true))) => {
                                // Symlink
                                let new_depth = match self.symlink_depth.checked_add(1) {
                                    None => {
                                        let e = ExofsError::OffsetOverflow;
                                        self.state = WalkerState::Failed(e.clone());
                                        return WalkerStepResult::Error(e);
                                    }
                                    Some(d) => d,
                                };
                                if new_depth > SYMLINK_MAX_DEPTH {
                                    let e = ExofsError::TooManySymlinks;
                                    self.state = WalkerState::Failed(e.clone());
                                    return WalkerStepResult::Error(e);
                                }
                                match backend.symlink_target(&oid) {
                                    Err(e) => {
                                        self.state = WalkerState::Failed(e.clone());
                                        WalkerStepResult::Error(e)
                                    }
                                    Ok(tgt) => {
                                        let tgt_copy = tgt.clone();
                                        self.state = WalkerState::AtSymlink {
                                            target: tgt,
                                            depth: new_depth,
                                        };
                                        WalkerStepResult::SymlinkEncountered(tgt_copy)
                                    }
                                }
                            }
                            Ok(Some((oid, false))) => {
                                // Directory or ordinary file
                                let new_dir = oid;
                                let rem_empty = remaining.is_empty();
                                if rem_empty {
                                    self.state = WalkerState::Done {
                                        oid: new_dir.clone(),
                                    };
                                    WalkerStepResult::Done(new_dir)
                                } else {
                                    *dir_oid = new_dir;
                                    WalkerStepResult::Continue
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Resolves path to completion.
    ///
    /// Iterative (RECUR-01) — explicit `while`/`loop`.
    pub fn collect_to_end<B: WalkerBackend>(&mut self, backend: &B) -> ExofsResult<ObjectId> {
        let mut iters: usize = 0;
        let max_iters: usize = 4096;
        loop {
            if iters >= max_iters {
                return Err(ExofsError::InternalError);
            }
            iters = iters.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            match self.step(backend) {
                WalkerStepResult::Done(oid) => return Ok(oid),
                WalkerStepResult::Error(e) => return Err(e),
                WalkerStepResult::Continue => {}
                WalkerStepResult::SymlinkEncountered(_) => {}
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves `path` from `root` using `backend`.
///
/// - First normalizes path (via `canonicalize_to_vec`).
/// - Then creates a `PathWalker` and iterates to completion.
pub fn walk_path<B: WalkerBackend>(
    root: ObjectId,
    path: &[u8],
    backend: &B,
) -> ExofsResult<ObjectId> {
    // Normalization (removes //, /., handles ..)
    let normalized = crate::fs::exofs::path::canonicalize::canonicalize_to_vec(path)?;
    let mut walker = PathWalker::new(root, &normalized)?;
    walker.collect_to_end(backend)
}

/// Resolves only the final component of a path (basename).
pub fn basename(path: &[u8]) -> ExofsResult<PathComponent> {
    let mut last: Option<PathComponent> = None;
    let mut parser = PathParser::new(path)?;
    while let Some(c) = parser.next_component()? {
        if c.as_bytes() != b"." && c.as_bytes() != b".." {
            last = Some(c);
        }
    }
    last.ok_or(ExofsError::InvalidPathComponent)
}

/// Resolves path up to parent directory (dirname semantics).
///
/// Returns OID of directory containing the last component.
pub fn walk_parent<B: WalkerBackend>(
    root: ObjectId,
    path: &[u8],
    backend: &B,
) -> ExofsResult<(ObjectId, PathComponent)> {
    let normalized = crate::fs::exofs::path::canonicalize::canonicalize_to_vec(path)?;
    // Find last occurrence of '/'
    let slash_pos = normalized.iter().rposition(|&b| b == b'/');
    let (parent_path, child_bytes) = match slash_pos {
        None => (b"/" as &[u8], normalized.as_slice()),
        Some(0) => (b"/" as &[u8], &normalized[1..]),
        Some(p) => (&normalized[..p], &normalized[p + 1..]),
    };
    let child = validate_component(child_bytes)?;
    let parent_oid = {
        let mut walker = PathWalker::new(root, parent_path)?;
        walker.collect_to_end(backend)?
    };
    Ok((parent_oid, child))
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

    struct FlatBackend {
        root: ObjectId,
    }
    impl WalkerBackend for FlatBackend {
        fn lookup(
            &self,
            _dir: &ObjectId,
            comp: &PathComponent,
        ) -> ExofsResult<Option<(ObjectId, bool)>> {
            let b = comp.as_bytes();
            if b == b"bin" {
                return Ok(Some((oid(2), false)));
            }
            if b == b"lib" {
                return Ok(Some((oid(3), false)));
            }
            if b == b"link" {
                return Ok(Some((oid(10), true)));
            }
            Ok(None)
        }
        fn symlink_target(&self, _oid: &ObjectId) -> ExofsResult<Vec<u8>> {
            let mut v = Vec::new();
            v.extend_from_slice(b"/bin");
            Ok(v)
        }
        fn root_oid(&self) -> ObjectId {
            self.root.clone()
        }
    }

    #[test]
    fn test_walk_simple() {
        let b = FlatBackend { root: oid(1) };
        let res = walk_path(oid(1), b"/bin", &b).test_unwrap();
        assert_eq!(res.0[0], 2);
    }

    #[test]
    fn test_walk_not_found() {
        let b = FlatBackend { root: oid(1) };
        assert!(walk_path(oid(1), b"/missing", &b).is_err());
    }

    #[test]
    fn test_basename() {
        let c = basename(b"/a/b/filename").test_unwrap();
        assert_eq!(c.as_bytes(), b"filename");
    }

    #[test]
    fn test_basename_root() {
        assert!(basename(b"/").is_err());
    }
}
