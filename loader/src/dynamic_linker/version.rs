//! Minimal ELF symbol versioning metadata placeholder (GNU `DT_VERSYM` / `DT_VERDEF`).

/// Checks whether two optional version strings are compatible.
/// Unversioned objects match any symbol binding per SVR4 defaults.
#[inline]
pub fn version_matches(requested: Option<&str>, candidate: Option<&str>) -> bool {
    match (requested, candidate) {
        (None, _) | (_, None) => true,
        (Some(a), Some(b)) => a == b,
    }
}
