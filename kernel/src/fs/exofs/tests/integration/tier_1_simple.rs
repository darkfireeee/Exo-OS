use crate::fs::exofs::cache::object_cache::ObjectCache;
use crate::fs::exofs::core::BlobId;

#[test]
fn test_cache_init() {
    let cache = ObjectCache::new_const();
    assert_eq!(cache.n_entries(), 0);
}

#[test]
fn test_cache_miss() {
    let cache = ObjectCache::new_const();
    let id = BlobId::from_raw([1u8; 32]);
    let result = cache.get(&id);
    assert!(result.is_none());
}
