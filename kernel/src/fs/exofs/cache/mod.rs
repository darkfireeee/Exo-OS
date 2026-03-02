//! Module cache/ — couches de cache ExoFS (no_std).

pub mod blob_cache;
pub mod cache_eviction;
pub mod cache_policy;
pub mod cache_pressure;
pub mod cache_shrinker;
pub mod cache_stats;
pub mod cache_warming;
pub mod extent_cache;
pub mod metadata_cache;
pub mod object_cache;
pub mod path_cache;

pub use blob_cache::{BlobCache, BLOB_CACHE};
pub use cache_eviction::{EvictionPolicy, EvictionAlgorithm};
pub use cache_policy::{CachePolicy, CacheConfig};
pub use cache_pressure::{CachePressure, CACHE_PRESSURE};
pub use cache_shrinker::{CacheShrinker, CACHE_SHRINKER};
pub use cache_stats::{CacheStats, CACHE_STATS};
pub use cache_warming::{CacheWarmer, WarmingStrategy};
pub use extent_cache::{ExtentCache, ExtentEntry, EXTENT_CACHE};
pub use metadata_cache::{MetadataCache, METADATA_CACHE};
pub use object_cache::{ObjectCache, CachedObject, OBJECT_CACHE};
pub use path_cache::{PathCache, PATH_CACHE};
