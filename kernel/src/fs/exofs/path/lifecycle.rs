//! ExoFS path module lifecycle (`init`, `shutdown`, `verify_health`) and tests.
//!
//! Extracted from `exofs/path/mod.rs` so `mod.rs` remains a pure wiring facade.

use super::{
    init_namespaces, init_path_cache, path_index, MOUNT_TABLE, MOUNT_TABLE_MAX, NAMESPACE_TABLE,
    NAMESPACE_TABLE_MAX, PATH_CACHE,
};

pub fn init(fs_root_oid: crate::fs::exofs::core::ObjectId) {
    path_index::ensure_mount_key_initialized();
    init_path_cache();
    let _ = init_namespaces(fs_root_oid);
}

pub fn shutdown() {
    PATH_CACHE.flush_all();
    MOUNT_TABLE.flush();
    NAMESPACE_TABLE.flush_non_root();
}

pub fn verify_health() -> bool {
    let cache_ok = PATH_CACHE.active_count() <= 256;
    let mounts_ok = MOUNT_TABLE.count() <= MOUNT_TABLE_MAX;
    let ns_ok = NAMESPACE_TABLE.count() <= NAMESPACE_TABLE_MAX;
    cache_ok && mounts_ok && ns_ok
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::{canonicalize_path, lookup_mount, validate_component, PATH_MAX};
    use super::*;
    use crate::fs::exofs::core::ObjectId;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    #[test]
    fn test_canonicalize_reexport() {
        let mut buf = [0u8; PATH_MAX];
        let n = canonicalize_path(b"/a/../b", &mut buf).test_unwrap();
        assert_eq!(&buf[..n], b"/b");
    }

    #[test]
    fn test_validate_component_reexport() {
        validate_component(b"hello").test_unwrap();
        assert!(validate_component(b"").is_err());
        assert!(validate_component(b"bad/comp").is_err());
    }

    #[test]
    fn test_health_initial() {
        assert!(verify_health());
    }

    #[test]
    fn test_mount_lookup_missing() {
        assert!(lookup_mount(&oid(200)).is_none());
    }
}
