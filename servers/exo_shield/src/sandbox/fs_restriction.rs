//! Filesystem restriction engine for exo_shield sandbox.
//!
//! Provides path whitelist / blacklist (max 32 entries each), access-mode
//! enforcement (read / write / execute), and glob-style wildcard matching.

use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of entries in either whitelist or blacklist.
pub const MAX_PATH_ENTRIES: usize = 32;

/// Maximum length of a single path pattern in bytes.
pub const MAX_PATH_LEN: usize = 128;

// ---------------------------------------------------------------------------
// Access mode
// ---------------------------------------------------------------------------

/// Bitfield describing which file-access modes are permitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AccessMode(u8);

impl AccessMode {
    pub const NONE: u8 = 0x00;
    pub const READ: u8 = 0x01;
    pub const WRITE: u8 = 0x02;
    pub const EXECUTE: u8 = 0x04;
    pub const ALL: u8 = 0x07;

    #[inline]
    pub const fn new(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    #[inline]
    pub const fn is_read(self) -> bool {
        self.0 & Self::READ != 0
    }

    #[inline]
    pub const fn is_write(self) -> bool {
        self.0 & Self::WRITE != 0
    }

    #[inline]
    pub const fn is_execute(self) -> bool {
        self.0 & Self::EXECUTE != 0
    }

    /// Check whether `requested` is a subset of the allowed modes.
    #[inline]
    pub const fn allows(self, requested: AccessMode) -> bool {
        (self.0 & requested.0) == requested.0
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Path entry
// ---------------------------------------------------------------------------

/// A single path pattern together with the access mode it governs.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct PathEntry {
    /// Glob-style path pattern (e.g. `/usr/lib/**`), null-terminated.
    pattern: [u8; MAX_PATH_LEN],
    /// Effective length of the pattern (excluding trailing nul padding).
    len: u16,
    /// Access mode this entry allows.
    mode: AccessMode,
    /// Whether the entry is in use.
    active: bool,
}

impl PathEntry {
    pub const fn empty() -> Self {
        Self {
            pattern: [0u8; MAX_PATH_LEN],
            len: 0,
            mode: AccessMode::new(AccessMode::NONE),
            active: false,
        }
    }

    /// Initialise from a byte slice.  Returns `None` if the slice is too
    /// long or empty.
    pub fn from_bytes(pattern: &[u8], mode: AccessMode) -> Option<Self> {
        if pattern.is_empty() || pattern.len() >= MAX_PATH_LEN {
            return None;
        }
        let mut buf = [0u8; MAX_PATH_LEN];
        let mut i = 0;
        while i < pattern.len() {
            buf[i] = pattern[i];
            i += 1;
        }
        Some(Self {
            pattern: buf,
            len: pattern.len() as u16,
            mode,
            active: true,
        })
    }

    #[inline]
    pub fn pattern_str(&self) -> &[u8] {
        &self.pattern[..self.len as usize]
    }

    #[inline]
    pub fn mode(&self) -> AccessMode {
        self.mode
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// Path matcher — supports `*` (any segment) and `**` (any depth)
// ---------------------------------------------------------------------------

/// Glob-style path matcher operating on byte slices (no alloc).
pub struct PathMatcher;

impl PathMatcher {
    /// Test whether `path` matches `pattern`.
    ///
    /// Supported wildcards:
    ///   - `*`  matches any sequence of characters **except** `/`
    ///   - `**` matches any sequence including `/`
    ///   - `?`  matches exactly one character (not `/`)
    pub fn matches(pattern: &[u8], path: &[u8]) -> bool {
        Self::match_inner(pattern, 0, path, 0)
    }

    fn match_inner(pat: &[u8], pi: usize, path: &[u8], qi: usize) -> bool {
        let mut pi = pi;
        let mut qi = qi;

        loop {
            // Both exhausted → match
            if pi == pat.len() && qi == path.len() {
                return true;
            }
            // Pattern exhausted but path remains → no match
            if pi == pat.len() {
                return false;
            }
            // Check for `**` (double-star)
            if pi + 1 < pat.len() && pat[pi] == b'*' && pat[pi + 1] == b'*' {
                // Skip the `**`
                pi += 2;
                // Optionally skip a trailing `/` after `**`
                if pi < pat.len() && pat[pi] == b'/' {
                    pi += 1;
                }
                // Try matching the rest of the pattern at every position
                let mut k = qi;
                loop {
                    if Self::match_inner(pat, pi, path, k) {
                        return true;
                    }
                    if k == path.len() {
                        return false;
                    }
                    k += 1;
                }
            }
            // Path exhausted but pattern remains → only match if remaining
            // pattern is all `*`
            if qi == path.len() {
                while pi < pat.len() && pat[pi] == b'*' {
                    pi += 1;
                }
                return pi == pat.len();
            }
            // Single `*` — matches any run of non-`/` characters
            if pat[pi] == b'*' {
                pi += 1;
                let mut k = qi;
                loop {
                    if Self::match_inner(pat, pi, path, k) {
                        return true;
                    }
                    if k >= path.len() || path[k] == b'/' {
                        return false;
                    }
                    k += 1;
                }
            }
            // `?` — matches exactly one non-`/` character
            if pat[pi] == b'?' {
                if path[qi] == b'/' {
                    return false;
                }
                pi += 1;
                qi += 1;
                continue;
            }
            // Literal match
            if pat[pi] == path[qi] {
                pi += 1;
                qi += 1;
                continue;
            }
            // Mismatch
            return false;
        }
    }
}

// ---------------------------------------------------------------------------
// Filesystem restriction policy
// ---------------------------------------------------------------------------

/// Policy deciding whether to use whitelist-first or blacklist-first
/// evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum FsPolicy {
    /// Only paths on the whitelist are allowed; blacklist is advisory.
    WhitelistFirst,
    /// All paths are allowed unless on the blacklist; whitelist is advisory.
    BlacklistFirst,
    /// Both lists must agree (intersection).
    Strict,
}

/// Complete filesystem restriction configuration.
#[derive(Debug)]
#[repr(C)]
pub struct FsRestrictionConfig {
    /// Whitelist entries.
    whitelist: [PathEntry; MAX_PATH_ENTRIES],
    /// Number of active whitelist entries.
    whitelist_count: u32,
    /// Blacklist entries.
    blacklist: [PathEntry; MAX_PATH_ENTRIES],
    /// Number of active blacklist entries.
    blacklist_count: u32,
    /// Evaluation policy.
    policy: FsPolicy,
    /// Default access mode when no rule matches.
    default_mode: AccessMode,
    /// Global generation counter (bumped on every mutation).
    generation: AtomicU32,
}

impl FsRestrictionConfig {
    /// Create a new config with the given policy and default mode.
    pub const fn new(policy: FsPolicy, default_mode: AccessMode) -> Self {
        Self {
            whitelist: [PathEntry::empty(); MAX_PATH_ENTRIES],
            whitelist_count: 0,
            blacklist: [PathEntry::empty(); MAX_PATH_ENTRIES],
            blacklist_count: 0,
            policy,
            default_mode,
            generation: AtomicU32::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------------

    /// Add an entry to the whitelist.  Returns `false` if the table is full.
    pub fn add_whitelist(&mut self, entry: PathEntry) -> bool {
        if self.whitelist_count as usize >= MAX_PATH_ENTRIES {
            return false;
        }
        let idx = self.whitelist_count as usize;
        self.whitelist[idx] = entry;
        self.whitelist_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Add an entry to the blacklist.  Returns `false` if the table is full.
    pub fn add_blacklist(&mut self, entry: PathEntry) -> bool {
        if self.blacklist_count as usize >= MAX_PATH_ENTRIES {
            return false;
        }
        let idx = self.blacklist_count as usize;
        self.blacklist[idx] = entry;
        self.blacklist_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Remove the first whitelist entry whose pattern equals `pattern`.
    pub fn remove_whitelist(&mut self, pattern: &[u8]) -> bool {
        let count = self.whitelist_count as usize;
        for i in 0..count {
            if self.whitelist[i].is_active()
                && Self::pattern_eq(&self.whitelist[i], pattern)
            {
                // Shift remaining entries down
                for j in i..count.saturating_sub(1) {
                    self.whitelist[j] = self.whitelist[j + 1];
                }
                self.whitelist[count - 1] = PathEntry::empty();
                self.whitelist_count -= 1;
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Remove the first blacklist entry whose pattern equals `pattern`.
    pub fn remove_blacklist(&mut self, pattern: &[u8]) -> bool {
        let count = self.blacklist_count as usize;
        for i in 0..count {
            if self.blacklist[i].is_active()
                && Self::pattern_eq(&self.blacklist[i], pattern)
            {
                for j in i..count.saturating_sub(1) {
                    self.blacklist[j] = self.blacklist[j + 1];
                }
                self.blacklist[count - 1] = PathEntry::empty();
                self.blacklist_count -= 1;
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Evaluation
    // -----------------------------------------------------------------------

    /// Check whether `path` is accessible with `requested` mode.
    pub fn check_access(&self, path: &[u8], requested: AccessMode) -> bool {
        let wl_mode = self.lookup_whitelist(path);
        let bl_mode = self.lookup_blacklist(path);

        match self.policy {
            FsPolicy::WhitelistFirst => {
                // Whitelist grants; blacklist cannot override a grant.
                if let Some(m) = wl_mode {
                    return m.allows(requested);
                }
                // No whitelist match → fall back to default
                self.default_mode.allows(requested)
            }
            FsPolicy::BlacklistFirst => {
                // Blacklist blocks; if blacklisted, deny.
                if let Some(m) = bl_mode {
                    // Blacklist entry means "these modes are forbidden".
                    // If the requested mode overlaps, deny.
                    if m.bits() & requested.bits() != 0 {
                        return false;
                    }
                }
                // Check whitelist for specific grants
                if let Some(m) = wl_mode {
                    return m.allows(requested);
                }
                self.default_mode.allows(requested)
            }
            FsPolicy::Strict => {
                // Must pass both checks.
                let wl_ok = match wl_mode {
                    Some(m) => m.allows(requested),
                    None => self.default_mode.allows(requested),
                };
                let bl_ok = match bl_mode {
                    Some(m) => m.bits() & requested.bits() == 0,
                    None => true,
                };
                wl_ok && bl_ok
            }
        }
    }

    /// Return the generation counter (useful for cache invalidation).
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Return the current policy.
    pub fn policy(&self) -> FsPolicy {
        self.policy
    }

    /// Change the evaluation policy.
    pub fn set_policy(&mut self, policy: FsPolicy) {
        self.policy = policy;
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Number of active whitelist entries.
    pub fn whitelist_count(&self) -> u32 {
        self.whitelist_count
    }

    /// Number of active blacklist entries.
    pub fn blacklist_count(&self) -> u32 {
        self.blacklist_count
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Walk the whitelist and return the most-specific matching mode.
    fn lookup_whitelist(&self, path: &[u8]) -> Option<AccessMode> {
        let mut best: Option<AccessMode> = None;
        let mut best_specificity: usize = 0;
        for i in 0..self.whitelist_count as usize {
            let entry = &self.whitelist[i];
            if !entry.is_active() {
                continue;
            }
            if PathMatcher::matches(entry.pattern_str(), path) {
                // More-specific (longer) patterns win.
                let spec = entry.pattern_str().len();
                if spec > best_specificity {
                    best_specificity = spec;
                    best = Some(entry.mode());
                }
            }
        }
        best
    }

    /// Walk the blacklist and return the mode bits that are denied.
    fn lookup_blacklist(&self, path: &[u8]) -> Option<AccessMode> {
        let mut best: Option<AccessMode> = None;
        let mut best_specificity: usize = 0;
        for i in 0..self.blacklist_count as usize {
            let entry = &self.blacklist[i];
            if !entry.is_active() {
                continue;
            }
            if PathMatcher::matches(entry.pattern_str(), path) {
                let spec = entry.pattern_str().len();
                if spec > best_specificity {
                    best_specificity = spec;
                    best = Some(entry.mode());
                }
            }
        }
        best
    }

    /// Byte-wise equality for a pattern entry vs a raw slice.
    fn pattern_eq(entry: &PathEntry, pattern: &[u8]) -> bool {
        let p = entry.pattern_str();
        p.len() == pattern.len() && {
            let mut i = 0;
            while i < p.len() {
                if p[i] != pattern[i] {
                    return false;
                }
                i += 1;
            }
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (inline, no_std-friendly)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_mode_allows_subset() {
        let rw = AccessMode::new(AccessMode::READ | AccessMode::WRITE);
        assert!(rw.allows(AccessMode::new(AccessMode::READ)));
        assert!(rw.allows(AccessMode::new(AccessMode::WRITE)));
        assert!(!rw.allows(AccessMode::new(AccessMode::EXECUTE)));
    }

    #[test]
    fn path_matcher_literal() {
        assert!(PathMatcher::matches(b"/usr/bin/ls", b"/usr/bin/ls"));
        assert!(!PathMatcher::matches(b"/usr/bin/ls", b"/usr/bin/cat"));
    }

    #[test]
    fn path_matcher_star() {
        assert!(PathMatcher::matches(b"/usr/*", b"/usr/bin"));
        assert!(!PathMatcher::matches(b"/usr/*", b"/usr/bin/ls"));
    }

    #[test]
    fn path_matcher_doublestar() {
        assert!(PathMatcher::matches(b"/usr/**", b"/usr/bin/ls"));
        assert!(PathMatcher::matches(b"/usr/**/ls", b"/usr/local/bin/ls"));
    }

    #[test]
    fn path_matcher_question() {
        assert!(PathMatcher::matches(b"/usr/b?n", b"/usr/bin"));
        assert!(!PathMatcher::matches(b"/usr/b?n", b"/usr/b/n"));
    }

    #[test]
    fn fs_restriction_whitelist_first() {
        let mut cfg = FsRestrictionConfig::new(FsPolicy::WhitelistFirst, AccessMode::new(AccessMode::NONE));
        let entry = PathEntry::from_bytes(b"/usr/bin/*", AccessMode::new(AccessMode::READ | AccessMode::EXECUTE)).unwrap();
        assert!(cfg.add_whitelist(entry));
        assert!(cfg.check_access(b"/usr/bin/ls", AccessMode::new(AccessMode::READ)));
        assert!(!cfg.check_access(b"/usr/bin/ls", AccessMode::new(AccessMode::WRITE)));
        assert!(!cfg.check_access(b"/etc/passwd", AccessMode::new(AccessMode::READ)));
    }

    #[test]
    fn fs_restriction_blacklist_first() {
        let mut cfg = FsRestrictionConfig::new(FsPolicy::BlacklistFirst, AccessMode::new(AccessMode::ALL));
        let entry = PathEntry::from_bytes(b"/etc/shadow", AccessMode::new(AccessMode::READ)).unwrap();
        assert!(cfg.add_blacklist(entry));
        assert!(!cfg.check_access(b"/etc/shadow", AccessMode::new(AccessMode::READ)));
        assert!(cfg.check_access(b"/etc/shadow", AccessMode::new(AccessMode::WRITE)));
    }

    #[test]
    fn fs_restriction_remove() {
        let mut cfg = FsRestrictionConfig::new(FsPolicy::WhitelistFirst, AccessMode::new(AccessMode::NONE));
        let entry = PathEntry::from_bytes(b"/tmp/*", AccessMode::new(AccessMode::ALL)).unwrap();
        assert!(cfg.add_whitelist(entry));
        assert_eq!(cfg.whitelist_count(), 1);
        assert!(cfg.remove_whitelist(b"/tmp/*"));
        assert_eq!(cfg.whitelist_count(), 0);
    }
}
