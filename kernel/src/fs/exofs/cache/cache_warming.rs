//! CacheWarmer — préchauffage du cache ExoFS au montage/démarrage (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::blob_cache::BLOB_CACHE;
use super::metadata_cache::METADATA_CACHE;

/// Stratégie de préchauffage.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmingStrategy {
    Disabled    = 0,
    SuperBlock  = 1,   // Charge uniquement les métadonnées racine.
    TopInodes   = 2,   // Charge les N inodes les plus accédés.
    FullReplay  = 3,   // Rejoue le journal d'accès récent.
}

/// Résultat du préchauffage.
#[derive(Debug, Default)]
pub struct WarmResult {
    pub blobs_loaded:   u32,
    pub inodes_loaded:  u32,
    pub bytes_loaded:   u64,
    pub errors:         u32,
}

/// Préchauffeur de cache.
pub struct CacheWarmer {
    strategy: WarmingStrategy,
    max_blobs: u32,
}

impl CacheWarmer {
    pub fn new(strategy: WarmingStrategy, max_blobs: u32) -> Self {
        Self { strategy, max_blobs: max_blobs.min(4096) }
    }

    pub fn default_superblock() -> Self {
        Self::new(WarmingStrategy::SuperBlock, 256)
    }

    /// Exécute le préchauffage depuis une liste de BlobIds à charger.
    pub fn warm_blobs(
        &self,
        blob_ids: &[BlobId],
        loader: &dyn BlobLoader,
    ) -> WarmResult {
        if self.strategy == WarmingStrategy::Disabled {
            return WarmResult::default();
        }

        let mut result = WarmResult::default();
        let limit = (self.max_blobs as usize).min(blob_ids.len());

        for bid in &blob_ids[..limit] {
            if BLOB_CACHE.get(bid).is_some() {
                // Déjà en cache.
                continue;
            }
            match loader.load_blob(bid) {
                Ok(data) => {
                    let size = data.len() as u64;
                    if BLOB_CACHE.insert(*bid, data).is_ok() {
                        result.blobs_loaded += 1;
                        result.bytes_loaded = result.bytes_loaded.saturating_add(size);
                    } else {
                        result.errors += 1;
                    }
                }
                Err(_) => { result.errors += 1; }
            }
        }

        result
    }

    pub fn strategy(&self) -> WarmingStrategy { self.strategy }
}

/// Trait pour charger un blob depuis le BlobStore.
pub trait BlobLoader {
    fn load_blob(&self, blob_id: &BlobId) -> Result<Vec<u8>, FsError>;
}
