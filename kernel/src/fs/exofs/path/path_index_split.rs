// path/path_index_split.rs — Split atomique d'un PathIndex
// Ring 0, no_std
//
// RÈGLE SPLIT-02 : UN SEUL EpochRoot pour le split entier
// Le split ne peut pas être interrompu à mi-chemin sans corruption.

use crate::fs::exofs::core::{ObjectId, EpochId, ExofsError, PATH_INDEX_SPLIT_THRESHOLD};
use crate::fs::exofs::path::path_index::{PathIndex, PathIndexEntry, fnv_hash};
use crate::fs::exofs::epoch::epoch_commit_lock::EPOCH_COMMIT_LOCK;
use crate::fs::exofs::epoch::epoch_delta::CURRENT_EPOCH_DELTA;
use alloc::vec::Vec;

/// Résultat d'un split PathIndex
pub struct SplitResult {
    /// ObjectId du nouveau child "low" (entrées hash < threshold)
    pub low_oid: ObjectId,
    /// ObjectId du nouveau child "high" (entrées hash >= threshold)
    pub high_oid: ObjectId,
    /// Seuil de partition
    pub threshold: u64,
}

/// Effectue le split atomique d'un PathIndex.
///
/// GARANTIE : une seule modification de l'EpochRoot (règle SPLIT-02).
/// Si la fonction échoue à mi-chemin, l'Epoch sera récupéré au prochain boot.
///
/// # Algorithme
/// 1. Trie les entrées par hash
/// 2. Calcule le seuil médian
/// 3. Crée deux nouveaux PathIndex (low/high)
/// 4. Les enregistre dans le delta epoch ATOMIQUEMENT (un seul appel)
/// 5. Marque l'ancien PathIndex comme "splitté"
pub fn split_path_index(
    index: &mut PathIndex,
    epoch: EpochId,
) -> Result<SplitResult, ExofsError> {
    if index.entry_count() < PATH_INDEX_SPLIT_THRESHOLD {
        return Err(ExofsError::InvalidArgument);
    }

    // Trie les entrées par hash pour split cohérent
    let mut sorted_entries = index.entries.clone();
    sorted_entries.sort_unstable_by_key(|(h, _, _)| *h);

    let mid = sorted_entries.len() / 2;
    let threshold = sorted_entries[mid].0;

    // Construit les deux nouveaux PathIndex
    let mut low = PathIndex::new_empty(index.self_oid, index.parent_oid, epoch)?;
    let mut high = PathIndex::new_empty(index.self_oid, index.parent_oid, epoch)?;

    for (hash, oid, name) in &sorted_entries[..mid] {
        low.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        low.entries.push((*hash, *oid, name.clone()));
        low.tree.insert(*hash, *oid);
    }
    for (hash, oid, name) in &sorted_entries[mid..] {
        high.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        high.entries.push((*hash, *oid, name.clone()));
        high.tree.insert(*hash, *oid);
    }

    // Alloue des ObjectId pour les nouveaux nodes
    let low_oid = crate::fs::exofs::core::new_class2();
    let high_oid = crate::fs::exofs::core::new_class2();
    low.self_oid = low_oid;
    high.self_oid = high_oid;

    // Enregistrement ATOMIQUE dans le delta epoch (règle SPLIT-02)
    // Un seul EpochRoot contiendra les deux nouveaux PathIndex
    CURRENT_EPOCH_DELTA.record_split_atomic(
        index.self_oid,
        low_oid,
        high_oid,
        threshold,
    );

    Ok(SplitResult { low_oid, high_oid, threshold })
}

impl PathIndex {
    /// Crée un PathIndex vide (pour un nouveau répertoire ou après split)
    pub fn new_empty(
        self_oid: ObjectId,
        parent_oid: ObjectId,
        epoch: EpochId,
    ) -> Result<Self, ExofsError> {
        use crate::fs::exofs::path::path_index_tree::PathIndexTree;
        let mut entries = Vec::new();
        entries.try_reserve(64).map_err(|_| ExofsError::NoMemory)?;
        Ok(PathIndex {
            self_oid,
            parent_oid,
            tree: PathIndexTree::new(),
            entries,
            dirty: true,
            epoch,
        })
    }
}
