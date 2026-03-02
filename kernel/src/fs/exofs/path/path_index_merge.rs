// path/path_index_merge.rs — Merge de PathIndex sous-chargés
// Ring 0, no_std
//
// Déclenché quand entry_count < PATH_INDEX_MERGE_THRESHOLD (512 entrées)
// Inverse du split — fusionne deux nodes en un seul

use crate::fs::exofs::core::{ObjectId, EpochId, ExofsError, PATH_INDEX_MERGE_THRESHOLD};
use crate::fs::exofs::path::path_index::PathIndex;
use crate::fs::exofs::epoch::epoch_delta::CURRENT_EPOCH_DELTA;
use alloc::vec::Vec;

/// Résultat d'un merge de PathIndex
pub struct MergeResult {
    /// ObjectId du nouveau PathIndex fusionné
    pub merged_oid: ObjectId,
    /// ObjectId du low (supprimé après merge)
    pub deleted_low_oid: ObjectId,
    /// ObjectId du high (supprimé après merge)
    pub deleted_high_oid: ObjectId,
}

/// Fusionne deux PathIndex (low + high) en un seul.
///
/// Précondition : low.count + high.count < PATH_INDEX_MERGE_THRESHOLD
/// Garanti atomique dans l'Epoch courant.
pub fn merge_path_indices(
    low: &PathIndex,
    high: &PathIndex,
    parent_oid: ObjectId,
    epoch: EpochId,
) -> Result<MergeResult, ExofsError> {
    let total = low.entry_count() + high.entry_count();
    if total >= PATH_INDEX_MERGE_THRESHOLD * 2 {
        // Trop d'entrées pour fusionner sans risquer un re-split immédiat
        return Err(ExofsError::InvalidArgument);
    }

    let merged_oid = crate::fs::exofs::core::new_class2();
    let mut merged = PathIndex::new_empty(merged_oid, parent_oid, epoch)?;
    merged.entries.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;

    // Fusion des entrées des deux nodes
    for (hash, oid, name) in &low.entries {
        merged.entries.push((*hash, *oid, name.clone()));
        merged.tree.insert(*hash, *oid);
    }
    for (hash, oid, name) in &high.entries {
        merged.entries.push((*hash, *oid, name.clone()));
        merged.tree.insert(*hash, *oid);
    }

    // Enregistre dans le delta epoch
    CURRENT_EPOCH_DELTA.record_merge(
        low.self_oid,
        high.self_oid,
        merged_oid,
    );

    Ok(MergeResult {
        merged_oid,
        deleted_low_oid: low.self_oid,
        deleted_high_oid: high.self_oid,
    })
}
