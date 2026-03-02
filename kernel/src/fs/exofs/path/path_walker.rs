// path/path_walker.rs — Iterator-based walking de l'arbre de répertoires
// Ring 0, no_std
//
// RÈGLES :
//   • RECUR-01 : itératif uniquement, jamais récursif
//   • RECUR-04 : pile sur le heap (Vec), jamais sur la stack
//   • Utilise la pile BFS pour listdir récursif

use crate::fs::exofs::core::{ObjectId, EpochId, ExofsError};
use crate::fs::exofs::path::path_index::PathIndex;
use alloc::vec::Vec;
use alloc::collections::VecDeque;

/// Entrée retournée lors d'un walk
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// ObjectId de l'entrée
    pub oid: ObjectId,
    /// Nom (octets UTF-8)
    pub name: Vec<u8>,
    /// Type de l'objet
    pub kind: crate::fs::exofs::core::ObjectKind,
    /// Profondeur depuis la racine du walk
    pub depth: usize,
    /// ObjectId du répertoire parent
    pub parent_oid: ObjectId,
}

/// Walker itératif — parcours BFS de l'arbre de répertoires
///
/// Stocke une file de travail sur le heap (règle RECUR-04)
pub struct PathWalker {
    /// File BFS : (dir_oid, profondeur)
    queue: VecDeque<(ObjectId, usize)>,
    /// Entrées à retourner pour le répertoire courant
    pending: Vec<WalkEntry>,
    /// Epoch de cohérence
    epoch: EpochId,
    /// Profondeur maximale (0 = illimitée)
    max_depth: usize,
    /// Inclure les répertoires dans les résultats
    include_dirs: bool,
}

impl PathWalker {
    /// Crée un nouveau walker à partir d'un répertoire racine
    pub fn new(
        root_oid: ObjectId,
        epoch: EpochId,
        max_depth: usize,
        include_dirs: bool,
    ) -> Result<Self, ExofsError> {
        let mut queue = VecDeque::new();
        queue.try_reserve(64)?; // pas de méthode try pour VecDeque, on accept ENOMEM en practice
        queue.push_back((root_oid, 0));
        Ok(PathWalker {
            queue,
            pending: Vec::new(),
            epoch,
            max_depth,
            include_dirs,
        })
    }

    /// Retourne la prochaine entrée du walk, ou None si terminé
    pub fn next(&mut self) -> Result<Option<WalkEntry>, ExofsError> {
        // Retourne les entrées en attente d'abord
        if let Some(entry) = self.pending.pop() {
            return Ok(Some(entry));
        }

        // Récupère le prochain répertoire à traiter
        while let Some((dir_oid, depth)) = self.queue.pop_front() {
            // Charge le PathIndex du répertoire
            let index = PathIndex::load(dir_oid, self.epoch)?;

            self.pending.try_reserve(index.entry_count())
                .map_err(|_| ExofsError::NoMemory)?;

            for (_, oid, name) in &index.entries {
                use crate::fs::exofs::objects::object_loader::quick_kind;
                let kind = quick_kind(*oid, self.epoch)
                    .unwrap_or(crate::fs::exofs::core::ObjectKind::Blob);

                // Si c'est un répertoire et profondeur non atteinte → enqueue
                if kind == crate::fs::exofs::core::ObjectKind::PathIndex {
                    if self.max_depth == 0 || depth + 1 < self.max_depth {
                        self.queue.push_back((*oid, depth + 1));
                    }
                    if !self.include_dirs {
                        continue;
                    }
                }

                let mut entry_name = Vec::new();
                entry_name.try_reserve(name.len()).map_err(|_| ExofsError::NoMemory)?;
                entry_name.extend_from_slice(name);

                self.pending.push(WalkEntry {
                    oid: *oid,
                    name: entry_name,
                    kind,
                    depth,
                    parent_oid: dir_oid,
                });
            }

            if let Some(entry) = self.pending.pop() {
                return Ok(Some(entry));
            }
        }

        Ok(None)
    }
}
